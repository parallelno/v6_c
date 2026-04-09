# Floating Point

**Runtime module:** [float.asm](../runtime/float.asm) (~1,260 lines) | **Shared storage:** [ops.asm](../runtime/ops.asm)

v6c supports IEEE 754 single-precision (`float`) via a software floating-point library for the Intel 8080. All operations are implemented as runtime subroutine calls — the 8080 has no hardware FPU.

## Memory Format

A `float` occupies 4 bytes in little-endian order:

```
Byte+0: bits  7..0   mantissa low
Byte+1: bits 15..8   mantissa mid
Byte+2: bits 23..16  mantissa high[6:0] | exponent[0]
Byte+3: bits 31..24  exponent[7:1] | sign
```

Standard IEEE 754 bit layout: `[sign:1][exponent:8][mantissa:23]`.

| Field | Bits | Range |
|-------|------|-------|
| Sign | 1 | 0 = positive, 1 = negative |
| Exponent | 8 | Biased by 127; 0 = zero/denorm, 255 = inf/NaN |
| Mantissa | 23 | Implicit leading 1 for normalized values |

## Calling Convention

All float runtime routines use shared memory-resident operands:

| Location | Size | Purpose |
|----------|------|---------|
| `__op1` | 4 bytes | First operand and result |
| `__op2` | 4 bytes | Second operand |

Defined in [ops.asm](../runtime/ops.asm), shared with `mul32`, `div32`, and `shift32`.

**Input:** operands in `__op1` and `__op2`.
**Output:** result in `__op1`, `HL` = low 16 bits of `__op1`.
**Comparisons:** result in `HL` (0 or 1).

## Runtime Routines

### Arithmetic

| Symbol | Operation | Algorithm |
|--------|-----------|-----------|
| `__fadd` | `__op1 + __op2` | Align exponents, add/subtract mantissas, normalize |
| `__fsub` | `__op1 - __op2` | Flip sign of `__op2`, fall through to `__fadd` |
| `__fmul` | `__op1 × __op2` | XOR signs, add exponents (bias 126), 24-bit shift-and-add mantissa multiply, normalize |
| `__fdiv` | `__op1 / __op2` | XOR signs, subtract exponents (bias 126), 24-bit restoring division with overflow pre-check, normalize |

### Comparisons

All comparisons return `HL = 1` (true) or `HL = 0` (false).

| Symbol | Operation | Implementation |
|--------|-----------|----------------|
| `__feq` | `__op1 == __op2` | Unpack both, compare sign + exponent + mantissa |
| `__fne` | `__op1 != __op2` | Call `__feq`, invert result |
| `__flt` | `__op1 < __op2` | Unpack, compare: same-sign uses magnitude; different-sign uses sign bit |
| `__fle` | `__op1 <= __op2` | Swap operands, call `__fge` |
| `__fgt` | `__op1 > __op2` | Swap operands, call `__flt` |
| `__fge` | `__op1 >= __op2` | Call `__flt`, invert result |

### Conversions

| Symbol | Operation | Notes |
|--------|-----------|-------|
| `__itof` | `int16 → float` | HL = signed 16-bit input; result in `__op1` |
| `__ftoi` | `float → int16` | `__op1` = float input; result in HL (truncation toward zero) |

`__ftoi` saturates to `INT16_MAX` (0x7FFF) / `INT16_MIN` (0x8000) on overflow.

### Internal Helpers

| Symbol | Purpose |
|--------|---------|
| `__funpack1` | Unpack `__op1` → `__fa_s1` (sign), `__fa_e1` (exponent), `__fa_m1` (mantissa with implicit bit) |
| `__funpack2` | Unpack `__op2` → `__fa_s2`, `__fa_e2`, `__fa_m2` |
| `__fpack` | Pack `__fa_s1`, `__fa_e1`, `__fa_m1` → `__op1` |
| `__fnorm` | Normalize mantissa: shift left until bit 23 set, adjusting exponent; shift right if overflow |

### Internal Storage

| Symbol | Size | Purpose |
|--------|------|---------|
| `__fa_s1` | 1 byte | Unpacked sign of operand 1 |
| `__fa_e1` | 1 byte | Unpacked biased exponent of operand 1 |
| `__fa_m1` | 4 bytes | Unpacked mantissa of operand 1 (24-bit + overflow byte) |
| `__fa_s2` | 1 byte | Unpacked sign of operand 2 |
| `__fa_e2` | 1 byte | Unpacked biased exponent of operand 2 |
| `__fa_m2` | 4 bytes | Unpacked mantissa of operand 2 |
| `__fa_tmp` | 8 bytes | Scratch for multiply accumulator / divide quotient |

## Code Generation

The compiler treats `float` as width `W32` (4 bytes). Float values always reside in memory spill slots — they never fit in 8080 registers.

### W32 Memory Model

| Operation | Generated Code |
|-----------|----------------|
| Load immediate | `LXI H,<low16>; SHLD label; LXI H,<high16>; SHLD label+2` |
| Load global | `LHLD global; SHLD spill; LHLD global+2; SHLD spill+2` |
| Store global | `LHLD spill; SHLD global; LHLD spill+2; SHLD global+2` |
| Arithmetic | Copy both operands to `__op1`/`__op2`; `CALL __f<op>`; copy `__op1` to result spill |
| Negation | Copy to spill; XOR byte 3 with `0x80` (sign-bit flip) |
| Cast int→float | `LHLD src; CALL __itof; LHLD __op1; SHLD spill; LHLD __op1+2; SHLD spill+2` |
| Cast float→int | Copy spill to `__op1`; `CALL __ftoi` (result in HL) |
| Compare | Copy to `__op1`/`__op2`; `CALL __f<cmp>`; result in HL |

### Spill Labels

Each W32 intermediate value gets a dedicated 4-byte spill slot (`__spill_N`). The register allocator's `alloc_spill_label()` and `mark_in_memory()` place W32 values directly in memory without attempting register allocation.

References to the high half use `__spill_N+2`. The `+` character is recognized by the spill label scanner so that `__spill_N+2` is not treated as a separate label.

## Constant Folding

The IR optimizer folds float operations at compile time when both operands are constants:

| Operation | Compile-Time Rule |
|-----------|--------------------|
| Add, Sub, Mul, Div | Standard IEEE 754 via Rust's `f32` operations, stored as `i64` bit pattern |
| Negation | XOR bit 31 of the IEEE 754 bit pattern (sign-bit flip, not integer negate) |
| Comparisons | Rust `f32` comparison, result is 0 or 1 |
| Cast int→float | Rust `as f32`, then `to_bits()` |
| Cast float→int | Rust `f32::from_bits()`, then `as i16 as i64` |

## Type System Integration

`float` is a first-class type in the v6c type system:

- `CType::Float` — 4 bytes, `is_float() = true`, `is_arithmetic() = true`, `is_scalar() = true`
- Parser recognizes `float` as a type keyword (including in casts: `(float)expr`)
- Implicit promotion: `int` is promoted to `float` in mixed arithmetic via `__itof`
- Explicit casts: `(float)i` and `(int)f` both supported

## Test Coverage

10 golden test files in [tests/unit/float/](../tests/unit/float/), all executed through the pipeline:
`source.c → v6c → .asm → v6asm → .rom → v6emul → verify HL==0`

| Test | Checks | Focus |
|------|:------:|-------|
| `float_arith.c` | 10 | Add, sub, mul, div — positive, negative, mixed signs |
| `float_cmp.c` | 16 | All 6 relational operators, sign edge cases |
| `float_conv.c` | 9 | int↔float round-trips, truncation, implicit promotion |
| `float_edge.c` | 8 | Zero identity, multiply by 1, large exponent difference |
| `float_chain.c` | 4 | Chained expressions, mixed int/float, multiple casts |
| `float_neg.c` | 7 | Unary negation, double-negate, negate+arithmetic, negate zero |
| `float_loop.c` | 4 | While-loop accumulation: sum, product, countdown, repeated divide |
| `float_cond.c` | 6 | if/else branching on float comparisons, nested conditions |
| `float_pow2.c` | 8 | Power-of-two squares, exact division, add/sub |
| `float_range.c` | 7 | Large values (10,000+), negative large, chained multi-step ops |

## Limitations

- **No `double`** — only `float` (32-bit) is supported; `double` is not implemented.
- **No denormals** — subnormal numbers are treated as zero.
- **No NaN/Inf propagation** — division by zero returns a saturated max-exponent value, not IEEE infinity.
- **Truncation only** — `__ftoi` truncates toward zero (no rounding modes).
- **No float arrays or struct fields** — `float arr[N]` and `struct { float f; }` are not yet supported.
- **No `printf("%f")`** — float-to-decimal formatting is not implemented.
- **Performance** — each float operation costs hundreds to thousands of 8080 cycles due to software implementation.
