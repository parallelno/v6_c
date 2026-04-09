# Floating-Point Support Design

## Summary

v6c provides IEEE 754 single-precision (32-bit) floating-point arithmetic
on the Vector-06c (Intel 8080 CPU) through a software library (`runtime/float.asm`)
and compiler-generated glue code.  The CPU has no FPU; every float operation
is implemented as an optimized 8080 assembly routine.

This document describes the architecture, data representation, public
interface, integration points, and testing strategy for the float subsystem.

---

## Design Goals

| # | Goal |
|---|------|
| G1 | **IEEE 754 compliance** — single-precision format (`float` = 4 bytes). Correct sign, exponent bias (127), implicit mantissa bit, ±0 semantics. |
| G2 | **Transparent C usage** — `float a = 1.5f; int x = (int)(a * 2.0f);` compiles with no user-visible assembly. |
| G3 | **Minimal footprint** — runtime is linked only when the program references float symbols; dead modules are excluded. |
| G4 | **Performance within 8-bit constraints** — register-pressure-aware routines, minimized memory traffic, early-exit paths for zero operands. |
| G5 | **Robustness** — correct handling of zero, sign, overflow, underflow, and division by zero. |
| G6 | **Testability** — every public routine is independently testable through the assembler/emulator toolchain. |

---

## Data Representation

### IEEE 754 Single-Precision Layout

```
  Bit  31  30..23   22..0
      ┌───┬────────┬───────────────────────┐
      │ S │  Exp   │       Mantissa        │
      └───┴────────┴───────────────────────┘
       1b    8b              23b
```

**Little-endian memory layout (4 bytes):**

| Byte Offset | Content |
|-------------|---------|
| `+0` | Mantissa bits 7..0 |
| `+1` | Mantissa bits 15..8 |
| `+2` | Mantissa bits 22..16 (7 bits) · Exponent bit 0 (1 bit) |
| `+3` | Exponent bits 7..1 (7 bits) · Sign (1 bit, MSB) |

- Exponent bias: 127 (exponent value 0 = denormalized/zero; 255 = infinity/NaN class).
- Implicit leading 1 in mantissa when exponent ≠ 0.
- ±0: all bits zero except possibly the sign bit; `+0 == -0`.

### Internal Unpacked Representation

During computation, each operand is decomposed into three scratch fields:

| Field | Size | Description |
|-------|------|-------------|
| `__fa_s{N}` | 1 byte | Sign (0 = positive, 1 = negative) |
| `__fa_e{N}` | 1 byte | Biased exponent (0..255) |
| `__fa_m{N}` | 4 bytes | Mantissa with implicit bit restored at bit 23, little-endian |

An additional 8-byte scratch area (`__fa_tmp`) is used by multiply and divide
for intermediate products and quotients.

---

## Module Architecture

```
 ┌──────────────────────────────────────────────────────────────┐
 │  C source                                                    │
 │  float a = 3.0f; float b = a + 1.5f; int x = (int)b;       │
 └──────────┬───────────────────────────────────────────────────┘
            │  parser / ir_gen
            ▼
 ┌──────────────────────────────────────────────────────────────┐
 │  IR: gen_float_binop(Add, a, 1.5f) → CALL __fadd            │
 │      cast float→int                → CALL __ftoi             │
 └──────────┬───────────────────────────────────────────────────┘
            │  codegen
            ▼
 ┌──────────────────────────────────────────────────────────────┐
 │  Assembly output:                                            │
 │    emit_w32_to_op1(a)      ; load lhs into __op1             │
 │    emit_w32_to_op2(1.5f)   ; load rhs into __op2             │
 │    CALL __fadd             ; invoke runtime                  │
 │    emit_op1_to_w32(tmp)    ; store result                    │
 │    ...                                                       │
 │    CALL __ftoi             ; float→int, result in HL         │
 └──────────┬───────────────────────────────────────────────────┘
            │  emit / runtime link
            ▼
 ┌──────────────────────────────────────────────────────────────┐
 │  Final .asm:                                                 │
 │    compiler-generated code                                   │
 │    ── appended only if referenced ──                         │
 │    runtime/float.asm (selective inclusion)                    │
 └──────────────────────────────────────────────────────────────┘
```

### Compiler Integration Points

| Component | File | Responsibility |
|-----------|------|----------------|
| **Lexer** | `lexer.rs` | Tokenizes `float` keyword (`TokenKind::Float`) and float literals (`TokenKind::FloatLiteral`) |
| **Parser** | `parser.rs` | Parses `float` type specifier and float literal expressions |
| **AST** | `ast.rs` | `ExprKind::FloatLiteral(f64)` node |
| **Type system** | `types.rs` | `CType::Float` — 4 bytes, `is_float()`, arithmetic promotion rules (`common_type`) |
| **IR generation** | `ir_gen.rs` | `gen_float_binop()` maps binary ops to library call names; inserts `__itof` / `__ftoi` for type casts |
| **Code generation** | `codegen.rs` | `gen_float_call()` — operand marshalling via `__op1`/`__op2`; `is_float_runtime_call()` identifies the 12 float symbols |
| **Runtime linker** | `runtime.rs` | `collect_runtime()` scans for CALL references; includes `float.asm` only when needed |
| **Emitter** | `emit.rs` | Appends runtime module assembly after compiler-generated code |

---

## Calling Convention

Float routines use a **global-operand** convention, consistent with the
project's 32-bit runtime pattern (shared with `div32`, `mul32`, `shift32`):

### Operand Storage

| Symbol | Size | Purpose |
|--------|------|---------|
| `__op1` | 4 bytes | First operand (input); result (output) |
| `__op2` | 4 bytes | Second operand (input) |

These are defined in the 32-bit runtime module and shared across all
32-bit operations.

### Arithmetic & Comparison Protocol

```
Caller:
  1. Store lhs (4 bytes) → __op1    (via LHLD/SHLD pairs)
  2. Store rhs (4 bytes) → __op2
  3. CALL __f<op>
  4a. Arithmetic: result in __op1; HL = low 16 bits of __op1
  4b. Comparison: HL = 0 or 1
```

### Conversion Protocol

| Function | Input | Output |
|----------|-------|--------|
| `__itof` | HL = signed int16 | `__op1` = float; HL = low 16 bits |
| `__ftoi` | `__op1` = float | HL = signed int16 (saturated on overflow) |

---

## Public Interface — Runtime Symbols

### Arithmetic

| Symbol | Operation | Inputs | Output |
|--------|-----------|--------|--------|
| `__fadd` | Addition | `__op1`, `__op2` | `__op1 = op1 + op2` |
| `__fsub` | Subtraction | `__op1`, `__op2` | `__op1 = op1 − op2` |
| `__fmul` | Multiplication | `__op1`, `__op2` | `__op1 = op1 × op2` |
| `__fdiv` | Division | `__op1`, `__op2` | `__op1 = op1 / op2` |

### Comparisons

| Symbol | Relation | Result in HL |
|--------|----------|--------------|
| `__feq` | `==` | 1 if equal, 0 otherwise |
| `__fne` | `!=` | 1 if not equal, 0 otherwise |
| `__flt` | `<` | 1 if less, 0 otherwise |
| `__fle` | `<=` | 1 if less or equal, 0 otherwise |
| `__fgt` | `>` | 1 if greater, 0 otherwise |
| `__fge` | `>=` | 1 if greater or equal, 0 otherwise |

### Conversions

| Symbol | Direction | Range/Saturation |
|--------|-----------|------------------|
| `__itof` | int16 → float | Exact for all int16 values |
| `__ftoi` | float → int16 | Saturates to `INT16_MIN`/`INT16_MAX` on overflow; 0 for tiny/zero |

---

## Internal Routines

These are not called from compiler-generated code but are used
internally by the public functions:

| Routine | Purpose |
|---------|---------|
| `__funpack1` | Decompose `__op1` → `__fa_s1`, `__fa_e1`, `__fa_m1` |
| `__funpack2` | Decompose `__op2` → `__fa_s2`, `__fa_e2`, `__fa_m2` |
| `__fpack` | Recompose `__fa_s1`, `__fa_e1`, `__fa_m1` → `__op1` |
| `__fnorm` | Normalize `__fa_m1`/`__fa_e1`: shift mantissa until bit 23 is the leading bit; handles both underflow (left shift) and overflow (right shift) |
| `__fiszero1` | Test `__op1 == ±0` (sets Z flag) |
| `__fiszero2` | Test `__op2 == ±0` (sets Z flag) |

### Shared Data Segment

| Label | Bytes | Usage |
|-------|-------|-------|
| `__fa_s1` | 1 | Unpacked sign of operand 1 |
| `__fa_e1` | 1 | Unpacked exponent of operand 1 |
| `__fa_m1` | 4 | Unpacked mantissa of operand 1 (with implicit bit) |
| `__fa_s2` | 1 | Unpacked sign of operand 2 |
| `__fa_e2` | 1 | Unpacked exponent of operand 2 |
| `__fa_m2` | 4 | Unpacked mantissa of operand 2 (with implicit bit) |
| `__fa_tmp` | 8 | Scratch for multiply (48-bit accumulator) and divide (quotient + remainder) |

**Total scratch: 22 bytes.**

---

## Algorithm Design

### Addition / Subtraction (`__fadd`, `__fsub`)

1. **Unpack** both operands.
2. **Zero checks** — if either operand is zero, return the other.
3. **Exponent alignment** — shift the mantissa of the smaller-exponent
   operand right by `|e1 − e2|`.  If the shift ≥ 25, the smaller operand
   contributes nothing (early return).
4. **Same sign** → add mantissas; **different signs** → subtract the
   smaller mantissa from the larger (compare 4-byte unsigned magnitudes).
   Switch result sign to the sign of the larger magnitude operand.
5. **Normalize** and **pack**.

`__fsub` is implemented as: flip sign bit of `__op2`, then fall through
to `__fadd`.

### Multiplication (`__fmul`)

1. **Unpack** both operands.
2. **Result sign** = `s1 XOR s2`.
3. **Zero check** — if either exponent is zero, result is zero.
4. **Result exponent** = `e1 + e2 − 127`.
5. **Mantissa multiply** — 24 × 24 bit shift-and-add into a 48-bit
   accumulator (`__fa_tmp`, 6 bytes).  Loop 24 iterations: test low bit
   of multiplier, conditionally add multiplicand to upper half of
   accumulator, shift accumulator right.
6. Extract top 24 bits of the 48-bit product as the result mantissa.
7. **Normalize** and **pack**.

### Division (`__fdiv`)

1. **Unpack** both operands.
2. **Result sign** = `s1 XOR s2`.
3. **Zero dividend** → return zero.  **Zero divisor** → return ±infinity
   (saturated exponent `0xFF`, zero mantissa, correct sign).
4. **Result exponent** = `e1 − e2 + 127`.
5. **Restoring division** — 24 iterations: shift remainder (mantissa) left,
   compare with divisor, subtract if ≥, shift quotient bit in.
6. Quotient becomes the result mantissa.
7. **Normalize** and **pack**.

### Comparisons

| Function | Strategy |
|----------|----------|
| `__feq` | Special-case `+0 == −0`; then byte-wise compare all 4 bytes |
| `__fne` | Call `__feq`, XOR result with 1 |
| `__flt` | Zero handling → sign comparison → same-sign magnitude comparison (with sign-dependent result inversion for negatives) |
| `__fle` | Swap operands, call `__flt`, negate result (`a ≤ b ⟺ ¬(b < a)`) |
| `__fgt` | Swap operands, call `__flt` (`a > b ⟺ b < a`) |
| `__fge` | Call `__flt`, negate result (`a ≥ b ⟺ ¬(a < b)`) |

### Integer ↔ Float Conversions

**`__itof` (int16 → float):**

1. Handle zero → return `+0.0`.
2. Save sign, negate if negative so magnitude is positive.
3. Place magnitude in mantissa bytes 0–1, clear bytes 2–3.
4. Set exponent to `150` (= 127 + 23).
5. Shift mantissa left by 8 (byte move), adjust exponent by −8.
6. Call `__fnorm` to finalize bit alignment, `__fpack` to produce IEEE 754.

**`__ftoi` (float → int16):**

1. Unpack.  Zero exponent → return 0.
2. Compute shift = `150 − exponent`.
3. If shift ≥ 24 → result is 0 (value too small).
4. If exponent > 150 → shift mantissa left; clamp to `INT16_MAX`/`INT16_MIN` on overflow.
5. Otherwise shift mantissa right.
6. Extract low 16 bits, apply sign.

---

## Performance Characteristics

Approximate cycle budgets on Vector-06c timings (not micro-benchmarked):

| Operation | Estimated Cycles | Notes |
|-----------|-----------------|-------|
| `__funpack` | ~120 | Per operand; extract sign/exp/mantissa |
| `__fpack` | ~80 | Reconstruct IEEE 754 from scratch fields |
| `__fnorm` | ~20–200 | Data-dependent: 0 iterations if already normalized, up to 23 left-shifts |
| `__fadd` (same sign, aligned) | ~400–600 | Best case: equal exponents |
| `__fadd` (general) | ~600–1,500 | Exponent alignment loop dominates |
| `__fmul` | ~2,000–3,000 | Fixed 24-iteration multiply loop |
| `__fdiv` | ~2,500–4,000 | Fixed 24-iteration restoring division |
| `__feq` / `__fne` | ~80–150 | Byte comparison, no unpack needed |
| `__flt` (general) | ~150–300 | Magnitude comparison, sign handling |
| `__itof` | ~300–500 | Unpack-free; normalize dominates |
| `__ftoi` | ~200–400 | Shift loop length depends on exponent |

### Optimization Strategies Used

- **Early exit on zero** — all arithmetic routines check for zero operands
  before unpacking, saving ~240+ cycles in the common identity cases.
- **Exponent dominance cutoff** — if exponent difference ≥ 25, the smaller
  operand contributes no precision and is skipped entirely.
- **`__fsub` reuse** — implemented as sign flip + `__fadd` (3 instructions).
- **Comparison reduction** — `__fle`, `__fgt`, `__fge` are all derived from
  `__flt` via operand swap and/or result negation, keeping the code for
  the core comparison in one place.
- **XCHG (4 cycles)** — used for register-pair swaps where applicable.
- **RAR/RLC (4 cycles)** — cheapest rotation instructions for bit extraction
  and multi-byte shifts.
- **Register B as accumulator** — reduces memory round-trips during
  multi-byte operations by keeping intermediate OR/ADD chains in B.

### Memory Footprint

| Component | Approximate Size |
|-----------|-----------------|
| Code (all 12 public + 6 internal routines) | ~1,200 bytes |
| Scratch data (`__fa_s1`..`__fa_tmp`) | 22 bytes |
| Shared operands (`__op1`, `__op2`) | 8 bytes (defined by `div32` module) |
| **Total** | **~1,230 bytes** |

The runtime is included only when at least one float symbol is referenced.

---

## Type System Integration

### C Type

```
CType::Float    size = 4 bytes    is_float() = true
```

### Arithmetic Promotion

The `common_type()` rule: if either operand of a binary expression is
`Float`, the result type is `Float`.  The non-float operand is implicitly
converted via `__itof`.

### Explicit Casts

- `(int)float_expr` → `__ftoi`
- `(float)int_expr` → `__itof`

### Limitations

| Limitation | Rationale |
|------------|-----------|
| No `double` | 8-byte soft-float would roughly double code/cycle cost on an 8-bit CPU with no practical benefit for this target |
| No `long` ↔ `float` conversion | `long` (32-bit int) is not yet a first-class type in v6c; `__itof`/`__ftoi` operate on int16 only |
| No NaN propagation | Full NaN semantics add significant branch complexity; current routines treat NaN bit patterns as ordinary values |
| No subnormal handling | Mantissa normalization skips when exponent = 0, producing flush-to-zero behavior for denormalized results |
| No rounding modes | All operations use truncation (round toward zero) |

---

## Runtime Module Registration

The float module is registered in `runtime.rs` as a self-contained unit
with no dependencies:

```
RuntimeModule {
    symbols: [
        "__fadd", "__fsub", "__fmul", "__fdiv",
        "__feq", "__fne", "__flt", "__fle", "__fgt", "__fge",
        "__itof", "__ftoi",
    ],
    asm: <embedded float.asm>,
    deps: [],
}
```

The `collect_runtime()` function scans the compiler-generated assembly for
`CALL __f*` / `CALL __itof` / `CALL __ftoi` references.  If none are
found, the entire float module is omitted from the final output.

All 12 public symbols and their internal helpers are included atomically
(no per-function granularity) because the internal routines
(`__funpack`, `__fpack`, `__fnorm`, etc.) are shared across operations.

---

## Dependency Map

```
float.asm (standalone — no dependencies)
    ├── defines:  __fadd __fsub __fmul __fdiv
    │             __feq __fne __flt __fle __fgt __fge
    │             __itof __ftoi
    ├── uses:     __op1, __op2 (4 bytes each, shared with div32/mul32/shift32)
    └── internal: __funpack1 __funpack2 __fpack __fnorm __fiszero1 __fiszero2
                  __fa_s1 __fa_e1 __fa_m1 __fa_s2 __fa_e2 __fa_m2 __fa_tmp

div32.asm
    └── defines:  __op1, __op2  (shared operand storage)
```

**Note:** `__op1`/`__op2` are the shared 32-bit operand slots defined by
`div32.asm`.  If a program uses float but not 32-bit integer division,
`float.asm` still needs these symbols.  The current runtime linker resolves
this by including the operand definitions as part of whichever 32-bit module
is first referenced.

---

## Code Generation Patterns

### Float Binary Operation (e.g., `c = a + b`)

```asm
; Load lhs (global float 'a') into __op1
LHLD _g_a          ; low 16 bits
SHLD __op1
LHLD _g_a+2        ; high 16 bits
SHLD __op1+2

; Load rhs (global float 'b') into __op2
LHLD _g_b
SHLD __op2
LHLD _g_b+2
SHLD __op2+2

; Call runtime
CALL __fadd

; Store result from __op1 to 'c'
LHLD __op1
SHLD _g_c
LHLD __op1+2
SHLD _g_c+2
```

### Float Literal Loading

Float literals are encoded at compile time as 32-bit IEEE 754 constants
and loaded via `LXI H, imm16` pairs:

```asm
; a = 3.0f  (IEEE 754: 0x40400000)
LXI H, 0x0000      ; low 16 bits
SHLD _g_a
LXI H, 0x4040      ; high 16 bits
SHLD _g_a+2
```

### Int-to-Float Cast

```asm
; float f = (float)int_var;
LHLD _l_main_int_var   ; int16 → HL
CALL __itof             ; result in __op1
LHLD __op1
SHLD _g_f
LHLD __op1+2
SHLD _g_f+2
```

### Float-to-Int Cast

```asm
; int x = (int)float_var;
LHLD _g_float_var
SHLD __op1
LHLD _g_float_var+2
SHLD __op1+2
CALL __ftoi             ; result in HL
SHLD _l_main_x
```

---

## Testing Strategy

### Golden Test Pipeline

Every float test follows the full **golden pipeline** — a C source file is
compiled, assembled, executed, and verified end-to-end:

```
 source.c ──(v6c)──► source.asm ──(v6asm)──► source.rom ──(v6emul)──► result
    │                    │                      │                      │
  compile            assemble               execute                verify
  v6c.exe            v6asm.exe              v6emul.exe            HL == 0
```

**Pass/fail convention:** `main()` returns 0 on success, N on check N failure.
The emulator prints the CPU state on halt; HL = 0x0000 means all checks passed.

### Test Levels

| Level | Scope | Toolchain | Location |
|-------|-------|-----------|----------|
| **Golden tests** | Full pipeline C → asm → rom → execute | v6c → v6asm → v6emul | `tests/unit/float/` |
| **Smoke tests** | Individual operations via C source | v6c → v6asm → v6emul | `temp/float_smoke*.c`, `temp/float_mul_test.c` |
| **Unit tests** | Per-routine correctness | v6asm → v6emul (pure ASM) | `tests/unit/` (planned) |
| **Integration tests** | End-to-end C compilation + execution | Rust harness (`tests/execution.rs`) | `tests/unit/` C files |

### Test Pattern

All float tests follow the project convention:

```c
float a, b, c;
int result;

int main(void) {
    a = <value>;
    b = <value>;
    c = a <op> b;
    result = (int)c;
    if (result != <expected>) return 1;   // test 1
    // ... more checks ...
    return 0;                              // all passed
}
```

- `return 0` = all checks passed.
- `return N` = check N failed.
- The Rust harness compiles, assembles, executes, and asserts `HL == 0`.

### Test Coverage Areas

| Category | What to Test |
|----------|-------------|
| **Arithmetic** | Add, sub, mul, div with positive, negative, mixed-sign, zero, and near-overflow values |
| **Comparisons** | All 6 relational operators; `+0 == −0`; sign edge cases |
| **Conversions** | `__itof`: 0, 1, −1, INT16_MAX, INT16_MIN; `__ftoi`: 0.0, 0.5 (truncation), large values (saturation), negative |
| **Edge cases** | Zero + zero, zero × nonzero, division by zero (infinity), very small exponent differences, exponent alignment cutoff |
| **Negation** | Unary minus, double negation, negate-then-operate, negate zero |
| **Loops** | Float accumulation (sum, product, repeated division) in while-loops — spill/reload stress |
| **Conditionals** | if/else branching driven by float comparisons, nested conditions |
| **Powers of two** | Exact squares, power-of-two division, exact add/sub — no rounding involved |
| **Large values** | Values in the 1 000–20 000 range, negative large, chained multi-step ops |
| **Round-trip** | `(int)(float)(int)x == x` for all representable int16 values |

### Tooling

| Tool | Path | Role |
|------|------|------|
| `v6c` | `target/debug/v6c.exe` | C compiler (generates `.asm` and `.lst`) |
| `v6asm` | `tools/v6asm/v6asm.exe` | Assembler (`.asm` → `.rom`). Docs: `tools/v6asm/docs/README.md` |
| `v6emul` | `tools/v6emul/v6emul.exe` | Emulator (headless test mode). Docs: `tools/v6emul/docs/README.md` |

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

**Golden test example (manual):**

```powershell
# Step 1: Compile C → ASM
.\target\debug\v6c.exe tests\unit\float\float_arith.c -o temp\float_arith.asm -l temp\float_arith.lst

# Step 2: Assemble ASM → ROM
.\tools\v6asm\v6asm.exe temp\float_arith.asm -o temp\float_arith.rom

# Step 3: Execute ROM, verify HL == 0
.\tools\v6emul\v6emul.exe --rom temp\float_arith.rom --load-addr 0x100 --halt-exit --dump-cpu --speed max --run-cycles 5000000
# Pass: H=00 L=00
```

The `--run-cycles` limit is set to 5M (vs the usual 1M) because float
operations are cycle-expensive (~1,500–4,000 cycles per operation).

The default starting execution address is `0x100` (matching the `.ORG`
directive emitted by `crt0.asm`), but `0x00` can be used for standalone
ASM-level unit tests.

---

## Future Considerations

| Area | Description |
|------|-------------|
| **`long` ↔ `float`** | When 32-bit integer support matures, add `__ltof` / `__ftol` conversion routines |
| **`double` type** | 8-byte IEEE 754 double-precision; significantly more expensive but may be needed for specific applications |
| **Constant folding** | Evaluate float expressions at compile time when both operands are literals, avoiding runtime calls entirely |
| **Peephole optimization** | Recognize patterns like `__itof` immediately followed by `__ftoi` (identity for representable values) and eliminate the pair |
| **Inline expansion** | For trivial cases (e.g., multiply by 1.0, add 0.0), emit identity code instead of a full runtime call |
| **Per-function linking** | Split `float.asm` into smaller modules (arithmetic, comparison, conversion) to reduce footprint when only a subset is used |
| **Rounding** | Implement IEEE 754 round-to-nearest-even for improved numerical accuracy |
