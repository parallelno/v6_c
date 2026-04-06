# Optimization Techniques from Hand-Optimized ASM — Applicability to v6c

**Date:** 2026-04-06  
**Source files:**
- `design/future_designs/refs/app_macros.asm` — pointer advance macros with validation
- `design/future_designs/refs/v6_macros.asm` — Vector-06C utility macros, 16-bit arithmetic idioms
- `design/future_designs/refs/v6_utils.asm` — memory copy/erase, palette, SP-based bulk ops

**Scope:** Cycle-count improvements for Vector-06C (i8080 with 4-cycle rounding).  
All cycle counts below use **Vector-06C rounded timing** unless marked *(i8080)*.

---

## 0. Vector-06C Cycle Rounding — Impact on Cost Model

The compiler's internal cost model (if any) must account for 4-cycle rounding.  
Several instructions become **more expensive relative to alternatives** on V6C than on stock i8080:

| Instruction  | i8080 cc | V6C cc | Note |
|-------------|----------|--------|------|
| MOV r,r      | 5        | 8      | +60% — makes register shuffling costlier |
| MVI r,n      | 7        | 8      | Nearly free difference vs MOV |
| INR/DCR r    | 5        | 8      | Same cost as MOV on V6C |
| INX/DCX rp   | 5        | 8      | Same cost as INR on V6C |
| ADD/SUB/AND/OR/XOR r | 4 | 4   | ALU ops are the cheapest class |
| ADI/SUI/ANI/ORI/XRI n | 7 | 8  | Immediate ALU nearly ties with reg ALU+MVI |
| DAD rp       | 10       | 12     | Relatively cheap 16-bit add |
| LXI rp,nn    | 10       | 12     | Same cost as DAD |
| MOV r,M / MOV M,r | 7   | 8     | Memory access = register move cost |
| LDA/STA addr | 13       | 16     | Direct memory = 2× MOV cost |
| LHLD/SHLD    | 16       | 16     | Same as LDA/STA pair |
| PUSH rp      | 11       | 12     | Cheap — critical for SP-based tricks |
| POP rp       | 10       | 12     | Cheap |
| CALL         | 17       | 20     | |
| RET          | 10       | 12     | |
| JMP / Jcc    | 10       | 12     | |
| XCHG         | 4        | 4      | Free — same as ALU op |
| CPI n        | 7        | 8      | |
| ORA r        | 4        | 4      | Preferred over CPI 0 (saves 4cc) |

**Key insight:** On V6C, `MOV r,r` (8cc) costs twice as much as `ADD r` (4cc). Any pattern that trades a MOV for an ALU op is a win. XCHG (4cc) is half the cost of MOV (8cc), making it essentially free.

---

## 1. Same-Page Pointer Advance via Low-Byte Arithmetic

### Technique (from `app_macros.asm`)

The `C_ADVANCE`, `E_ADVANCE`, `L_ADVANCE` macros advance a pointer by a known compile-time offset using only the **low byte** of a register pair, avoiding a full 16-bit add:

| Offset range   | Method                          | V6C cc |
|----------------|---------------------------------|--------|
| ±1             | `INR r` / `DCR r`               | 8      |
| ±2             | `INR r; INR r` / `DCR r; DCR r` | 16     |
| ±3..±127       | `MVI A,diff; ADD r; MOV r,A`    | 20     |

Compare to the compiler's current 16-bit pointer advance:

| Offset range   | Current codegen                  | V6C cc |
|----------------|----------------------------------|--------|
| ±1             | `INX H` / `DCX H`               | 8      |
| ±2–3           | `INX H; INX H` / etc.           | 16–24  |
| ≥4             | `LXI D,N; DAD D`                | 24     |

### Recommendation

**When the compiler can prove two consecutive pointer accesses lie within the same 256-byte page** (e.g., struct field accesses, local variables on a page-aligned stack frame, or array accesses with compile-time known indices), emit low-byte-only arithmetic on C, E, or L instead of full 16-bit `LXI+DAD`.

For offsets 3–127 in a same-page scenario:
- Current: `LXI D,N; DAD D` = **24cc**, clobbers DE
- Proposed: `MVI A,N; ADD L; MOV L,A` = **20cc**, clobbers only A

**Savings: 4cc per pointer advance, preserves DE.**

For consecutive struct field accesses where offsets are small, this accumulates quickly. A struct with 4 fields accessed sequentially saves ~16cc.

### Applicability

- **Struct field access chains** — very common pattern in C code
- **Stack-frame locals** — if the frame fits in 256 bytes (most functions)
- **Array access with known small index** — e.g., `arr[i+1]` after `arr[i]`

### Complexity: Low–Medium

Requires tracking whether HL (or BC/DE) points into a known 256-byte page. Could be done conservatively: if the base address is a stack pointer offset and frame size < 256, low-byte-only advance is safe.

---

## 2. `ADI`/`ACI`/`SUB` Idiom for rp = A + const16

### Technique (from `v6_macros.asm`)

The macros `HL_TO_A_PLUS_INT16`, `BC_TO_A_PLUS_INT16`, `DE_TO_AX2_PLUS_INT16` compute a 16-bit register pair from an 8-bit value in A plus a 16-bit constant:

```asm
; HL = A + int16_const  (36cc)
    adi <int16_const     ; A += lo(const), sets carry
    mov l, a             ; L = lo(result)
    aci >int16_const     ; A += hi(const) + carry
    sub l                ; A -= lo(result), giving hi(result)
    mov h, a             ; H = hi(result)
```

With a preceding `ADD A` for ×2 variants:

```asm
; HL = A*2 + int16_const  (40cc)
    add a                ; A *= 2
    adi <int16_const
    mov l, a
    aci >int16_const
    sub l
    mov h, a
```

And ×4 variant using `ADD A; ADD A`:

```asm
; HL = A*4 + int16_const  (44cc)
    add a                ; A *= 2
    add a                ; A *= 2 (total ×4)
    adi <int16_const
    mov l, a
    aci >int16_const
    sub l
    mov h, a
```

### Current compiler equivalent

For array indexing `base[idx]` where `idx` is W8 and element size is 1/2/4:

```asm
; Current: HL = base + (W8)idx  (~48–56cc typical)
    mov l, a             ; 8cc
    mvi h, 0             ; 8cc  (zero-extend)
    xchg                 ; 4cc
    lxi h, base          ; 12cc
    dad d                ; 12cc   Total: 44cc (size=1)
```

For size=2, add a `DAD H` before the `DAD D` → 56cc.

### Recommendation

**Emit the `ADI/ACI/SUB` pattern for `rp = (W8_reg * {1,2,4}) + const16`.**

| Operation            | Current  | Proposed | Savings |
|----------------------|----------|----------|---------|
| `HL = A + const16`   | 44cc     | 36cc     | **8cc** |
| `HL = A*2 + const16` | 56cc     | 40cc     | **16cc** |
| `HL = A*4 + const16` | 60cc     | 44cc     | **16cc** |

This is a **very common pattern** — it covers most `char[]`, `int[]`, and pointer array indexing from a W8 index variable.

### Applicability

- Array indexing: `arr[i]` where `i` is `char` or fits in 8 bits
- Switch/jump table address computation
- Struct array element access with small element sizes

### Complexity: Medium

Requires codegen to detect the pattern:  
`PtrAdd(dest, base_const, idx_w8, scale ∈ {1,2,4})` → emit the `ADI/ACI/SUB` sequence.  
Partially overlaps with existing `Mul(d, x, 2)` → `DAD H` but should be recognized as a **combined** scale+offset pattern.

---

## 3. `ORA A` Instead of `CPI 0` for Zero-Test

### Technique (from `v6_macros.asm`)

```asm
.macro CPI_ZERO(int8_const = 0)
    ora a            ; 4cc, sets Z flag from A
.endmacro
```

### Current compiler status

Peephole rule 5 already converts `ORA L; CPI 0` → `ORA L` when CPI follows ORA.  
However, standalone zero-tests may still emit `CPI 0` (8cc) instead of `ORA A` (4cc).

### Recommendation

**In codegen, whenever emitting a comparison of A with immediate 0, emit `ORA A` (4cc) instead of `CPI 0` (8cc).**

Also look for any case where an ALU operation already set the flags correctly and a redundant `ORA A` or `CPI 0` follows — eliminate it in peephole.

**Savings: 4cc per zero-test.** In loop termination checks, this is hit every iteration.

### Complexity: Low

---

## 4. SP-Based Bulk Memory Operations

### Technique (from `v6_utils.asm`)

**`mem_erase_sp`** — clears memory using repeated `PUSH B`:

```asm
mem_erase_sp:
    ; ... setup SP to point to end of buffer ...
    lxi b, $0000          ; fill value
    sphl
@loop:
    PUSH_B(16)            ; 16 × push b = 32 bytes per iteration
    dcx d
    ; ... loop check ...
```

Each `PUSH B` writes 2 bytes in 12cc = **6cc/byte**.  
Compare: `MOV M,r; INX H` loop = 8+8 = 16cc/byte. **2.67× faster**.

**`mem_copy_to_ram_disk`** — copies memory using `POP; PUSH` pattern through SP, with **Duff's device** (jump-table entry into unrolled loop) for handling non-aligned lengths.

### Recommendation

**For `memset` and `memcpy` with compile-time-known sizes, emit SP-based bulk operations when size ≥ ~8 bytes.**

The pattern:
1. Save SP: `LXI H,0; DAD SP; SHLD saved_sp`
2. Set SP to destination end
3. Load fill value into BC (for memset) or set source SP (for memcpy)
4. Unrolled PUSH/POP loop (16–32 operations per iteration)
5. Restore SP: `LHLD saved_sp; SPHL`

**Performance for `memset(buf, 0, 32)`:**

| Method               | V6C cc  |
|----------------------|---------|
| Byte loop            | ~512    |
| SP-based (16× PUSH)  | ~240    |

**Constraints:**
- Requires disabled interrupts (or saving/restoring interrupt state) since SP is repurposed
- Buffer alignment to 2-byte boundary for PUSH
- Setup cost (~40cc) means break-even at ~8 bytes

### Applicability

- `memset()` — struct zeroing, array initialization
- `memcpy()` — struct copies, buffer operations
- Compiler-generated struct copies on assignment

### Could be implemented as:

- Compiler intrinsic for `memset`/`memcpy` with known sizes
- Inline expansion in codegen for struct copies ≥ 8 bytes
- Runtime library routines for dynamic sizes (already partially exists)

### Complexity: Medium–High

Requires interrupt management, SP save/restore, and alignment handling.

---

## 5. Duff's Device — Jump-Table Loop Entry for Partial Unrolling

### Technique (from `v6_utils.asm`)

For `mem_copy_to_ram_disk` with non-aligned lengths, the code computes a **jump offset into the unrolled loop body**:

```asm
    mvi a, 0b00011110      ; mask = 30 (reminder of 32)
    ana c                   ; remainder = len & 30
    HL_TO_A_PLUS_INT16(jmp_tbl)  ; HL = jmp_tbl + remainder
    mov a, m                ; read jump target from table
    inx h
    mov h, m
    mov l, a
    shld @start_loop + 1    ; self-modify: patch the JMP target
```

A jump table maps remainders to entry points within the unrolled loop:

```asm
jmp_tbl:
    .word loop + WORD_LEN * 0   ; remainder 0 → full loop
    .word loop + WORD_LEN * 15  ; remainder 2 → skip 1 op
    .word loop + WORD_LEN * 14  ; remainder 4 → skip 2 ops
    ; ...
```

### Current compiler status

The compiler supports loop unrolling for small trip counts (≤8) and `#pragma unroll`, but does **not** implement Duff's device for partial unrolling of larger loops.

### Recommendation

**For loops with runtime trip count but unrolled body, emit Duff's device to handle the remainder:**

1. Unroll the loop body N times (e.g., 8 or 16)
2. Compute `remainder = count % N`
3. Use a jump table (or computed jump) to enter the unrolled body at the right offset
4. Continue with the main unrolled loop for full iterations

This is most applicable to:
- `memcpy`/`memset` library routines
- Compiler-generated array initialization loops
- Any loop where the trip count is not a compile-time constant but the body is simple

### Complexity: High

Requires the compiler to emit jump tables and remainder computation. Best implemented first in the runtime library (`memcpy`, `memset`) and later as a codegen pattern for simple loops.

---

## 6. Tiered Pointer Advance Strategy

### Technique (from `v6_macros.asm` — `HL_ADVANCE`)

The macro selects the cheapest advance method based on offset magnitude:

| Offset `d`    | Method                          | V6C cc | Clobbers |
|--------------|---------------------------------|--------|----------|
| 0            | nothing                         | 0      | — |
| ±1           | `INX H` / `DCX H`              | 8      | — |
| ±2           | `INX H; INX H`                 | 16     | — |
| ±3           | `INX H; INX H; INX H`          | 24     | — |
| ±4..±32767   | `LXI rp,d; DAD rp`             | 24     | BC or DE |
| arbitrary    | `MVI A,lo; ADD L; MOV L,A; MVI A,hi; ADC H; MOV H,A` | 40 | A |

### Current compiler status

The compiler uses:
- `INX H`/`DCX H` for ±1 (well handled)
- `LXI D,N; DAD D` for larger offsets

But **does not select the optimal tier** in all cases:
- `INX H × 2` (16cc) vs `LXI D,2; DAD D` (24cc) — the compiler may prefer the latter
- `INX H × 3` (24cc) ties with `LXI D,3; DAD D` (24cc) but doesn't clobber DE
- For offsets ±4 and up, `LXI+DAD` (24cc) is strictly better than `INX×4` (32cc)

### Recommendation

**Codegen should select the tier based on offset magnitude, with register-pressure awareness:**

```
if offset == 0: skip
if |offset| ≤ 3: emit INX/DCX × |offset|  (8–24cc, no clobber)
if |offset| ≤ 3 AND a register pair is free: still prefer INX/DCX (same cost, no clobber)
if |offset| ≥ 4: emit LXI rp,offset; DAD rp (24cc, clobbers rp)
```

**Also:** when DE is already dead (not holding a live value), prefer `LXI D` for the advance. When DE is live but BC is free, use `LXI B; DAD B`. The current codegen may not perform this register-pressure-aware selection.

### Complexity: Low–Medium

Mostly a codegen decision table. Register liveness info is already available.

---

## 7. `XRA A` for Zero with Flag Semantics

### Technique (from `v6_macros.asm`)

```asm
.macro A_TO_ZERO(int8_const, useXRA = true)
    .if useXRA
        xra a            ; 4cc, A=0, sets Z, clears CY
    .endif
    .if useXRA == false
        mvi a, 0         ; 8cc, A=0, flags unchanged
    .endif
.endmacro
```

The macro distinguishes between cases where:
- Flag clobbering is acceptable → `XRA A` (4cc)
- Flags must be preserved → `MVI A,0` (8cc)

### Current compiler status

Peephole rule 21 converts `MVI A,0` → `XRA A`. This is correct when flags are dead after the instruction.

### Recommendation

Ensure peephole rule 21 **checks flag liveness** before converting. If flags are live (e.g., before a conditional branch that depends on a previous comparison), `MVI A,0` must be preserved. If flags are dead, `XRA A` saves 4cc.

**Also:** extend this to `MVI r,0` for any register where an XRA/SUB equivalent exists — though only A supports `XRA A` on i8080, so this is A-specific.

### Complexity: Low

Already implemented; verify correctness of flag-liveness check.

---

## 8. Register-Pair Addition via Low+High Byte Chaining

### Technique (from `v6_macros.asm`)

```asm
; BC += HL  (40cc)
.macro BC_TO_BC_PLUS_HL()
    mov a, c        ; 8cc
    add l            ; 4cc
    mov c, a         ; 8cc
    mov a, b         ; 8cc
    adc h            ; 4cc
    mov b, a         ; 8cc
.endmacro
```

This adds any two register pairs where neither is HL (since `DAD` only adds *to* HL).

### Current compiler status

16-bit addition always routes through HL:
```asm
; BC += DE (current, ~48cc)
    mov h, b; mov l, c    ; 16cc
    dad d                  ; 12cc
    mov b, h; mov c, l    ; 16cc (+ possible XCHG)
```

### Recommendation

**When adding two non-HL register pairs and HL is live, emit the byte-chaining ADD/ADC pattern instead of saving/restoring HL.**

| Scenario            | Current (via HL) | Proposed (byte-chain) | Savings |
|---------------------|-------------------|-----------------------|---------|
| `BC += DE`, HL live | ~48cc            | 40cc                  | **8cc** |
| `BC += HL`          | N/A (already DAD) | 40cc                 | — |

This is most useful inside loops where HL holds a pointer and BC/DE are used as accumulators.

### Complexity: Low

A straightforward codegen alternative when HL is occupied.

---

## 9. 4-Byte Aligned Jump Tables

### Technique (from `v6_macros.asm`)

```asm
.macro JMP_4(DST_ADDR)
    jmp DST_ADDR     ; 3 bytes
    nop               ; 1 byte (pad to 4)
.endmacro
```

Aligning jump table entries to 4 bytes enables **indexed dispatch by shift** instead of multiplication.

### Current compiler status

Switch/case codegen is not well documented. Jump tables may not use alignment optimization.

### Recommendation

For **switch statements** compiled to jump tables, align entries to 4 bytes. This allows the dispatch index to be computed as `base + idx * 4`, which is implementable as `ADD A; ADD A; ADI lo; ...` using the technique from §2.

Without alignment, entries are 3 bytes (JMP = 3 bytes), requiring multiplication by 3 — more expensive.

### Complexity: Medium

Requires jump-table codegen to emit NOP padding and adjust index computation.

---

## 10. Comparison: Loop Cost Analysis

To illustrate cumulative impact, consider a simple byte-array traversal:

```c
char *p = buf;
for (int i = 0; i < n; i++) {
    process(*p);
    p++;
}
```

### Current codegen (estimated inner loop)

```asm
    lhld p           ; 16cc
    mov a, m          ; 8cc
    ; ... process(A) ...
    inx h             ; 8cc
    shld p            ; 16cc
    lhld i            ; 16cc
    inx h             ; 8cc
    shld i            ; 16cc
    ; compare i < n
    ; ... branch ...
    ; Total overhead: ~88cc + process + branch
```

### With techniques from this report

```asm
    ; HL kept in register (no LHLD/SHLD per iteration)
    mov a, m          ; 8cc
    ; ... process(A) ...
    inx h             ; 8cc
    ; loop counter in B, decrement
    dcr b             ; 8cc
    jnz loop          ; 12cc
    ; Total overhead: 36cc + process
```

**Savings: ~52cc per iteration** — primarily from keeping HL in a register across the loop and using a register-based loop counter. This is a register allocation improvement more than a single peephole, but the techniques in this report contribute to enabling it.

---

## Summary — Priority-Ordered Recommendations

| # | Technique | Savings/hit | Frequency | Effort | Priority |
|---|-----------|-------------|-----------|--------|----------|
| 2 | `ADI/ACI/SUB` for `rp = A*{1,2,4} + const16` | 8–16cc | Very High (array indexing) | Medium | **P0** |
| 1 | Same-page low-byte pointer advance | 4cc | High (struct access) | Low–Med | **P1** |
| 6 | Tiered INX/DCX vs LXI+DAD selection | 0–8cc | High | Low | **P1** |
| 3 | `ORA A` instead of `CPI 0` | 4cc | High (loop exits) | Low | **P1** |
| 8 | Byte-chain ADD/ADC for non-HL pair addition | 8cc | Medium | Low | **P2** |
| 4 | SP-based bulk memset/memcpy | 40–60% faster | Medium (libc, struct init) | Med–High | **P2** |
| 0 | V6C 4-cycle cost model in codegen | varies | All code | Medium | **P2** |
| 9 | 4-byte aligned jump tables | ~8cc dispatch | Low (switch) | Medium | **P3** |
| 5 | Duff's device for partial unrolling | ~20% loop overhead | Low | High | **P3** |
| 7 | Flag-aware `XRA A` / `MVI A,0` selection | 4cc | Low | Low | **P3** (verify existing) |
