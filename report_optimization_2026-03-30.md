# V6C Optimization Report — 2026-03-30

Detailed analysis of v6c compiler output across sieve.c, dhrystone.c, fannkuch.c,
loop.c, and the optimization_small test suite. Items are ordered by estimated
cycle-count impact (highest first).

---

## 1. W16 Comparison + Branch Fusion (CRITICAL)

**Current state:** CPI-branch fusion exists only for W8 comparisons. Every W16
comparison (the dominant width) materialises a full boolean value into HL,
then tests it again. A typical `i <= size` loop guard emits **~18 instructions /
~60 cycles**:

```asm
; --- current: i <= size ---
LHLD _l_main_i        ; load i
XCHG
LHLD _l_main_size     ; load size
MOV  B,D              ; shuffle registers
MOV  C,E
XCHG
MOV  H,B
MOV  L,C
MOV  A,H              ; high-byte compare
SUB  D
JNZ  __cmp_done___cg_0
MOV  A,L              ; low-byte compare
SUB  E
__cmp_done___cg_0:
JM   __cg_0           ; "true" path
JZ   __cg_0
LXI  H,0              ; materialize false
JMP  __cg_1
__cg_0:
LXI  H,1              ; materialize true
__cg_1:
MOV  B,H              ; copy to BC (dead)
MOV  C,L
MOV  A,H              ; test boolean
ORA  L
JZ   L1__main         ; branch
```

**Proposed:** Implement W16 CPI-branch fusion in `gen_compare`. When the next
IR op is `JumpIfTrue`/`JumpIfFalse` consuming the comparison result, skip
boolean materialisation entirely and emit a direct conditional jump:

```asm
; --- proposed: i <= size ---
LHLD _l_main_i
XCHG
LHLD _l_main_size
MOV  A,H
SUB  D
JNZ  __cmp_done
MOV  A,L
SUB  E
__cmp_done:
JM   L0__main         ; continue loop
JZ   L0__main
JMP  L1__main         ; exit loop
```

This cuts **~10 instructions / ~35 cycles per comparison** and eliminates
four register shuffles, the LXI+JMP materialisation pair, the MOV B,H / MOV C,L
copy, and the ORA L + JZ re-test, yielding **~60 % speedup on comparison
sequences**. Every loop in the benchmarks benefits.

The register shuffle (`MOV B,D; MOV C,E; XCHG; MOV H,B; MOV L,C`) exists
because `ensure_de(rhs)` followed by `ensure_hl(lhs)` sometimes evicts the
value that was just placed. The fusion avoids materialising into BC entirely,
so these shuffles also disappear.

---

## 2. PtrAdd element_size=2 Uses `CALL __mul16` (HIGH)

Every `int` array access generates `CALL __mul16` for `offset * 2`:

```asm
; flags[i] access:
LHLD _l_main_i
LXI  D,2
CALL __mul16       ; ~150 cycles for a simple shift!
XCHG
LXI  H,_g_flags
DAD  D
```

`__mul16` is a general-purpose 16-bit multiply loop (~150 cycles).
Multiplying by 2 should be a single `DAD H` (11 cycles).

**Proposed:** Extend `gen_ptr_add` with fast-paths matching `gen_mul`:

```rust
fn gen_ptr_add(&mut self, dst: VReg, ptr: VReg, offset: VReg, element_size: u16) {
    match element_size {
        1 => {
            self.ensure_de(offset);
            self.ensure_hl(ptr);
            self.emit_inst("DAD D");
        }
        2 | 4 | 8 => {
            self.ensure_hl(offset);
            let shifts = match element_size { 2 => 1, 4 => 2, _ => 3 };
            for _ in 0..shifts { self.emit_inst("DAD H"); }
            self.emit_inst("XCHG");
            self.ensure_hl(ptr);
            self.emit_inst("DAD D");
        }
        // ...existing __mul16 fallback...
    }
}
```

In sieve.c the inner loop has four `int[]` accesses → saves ~**560 cycles per
inner iteration**. In fannkuch.c, virtually every line uses array indexing.

---

## 3. Redundant Register Shuffles Around Compares (HIGH)

The current comparison code path does this:

```asm
LHLD _l_main_i    ; HL = i
XCHG               ; DE = i
LHLD _l_main_size ; HL = size
MOV  B,D           ; BC = i  (from DE)
MOV  C,E
XCHG               ; DE = size, HL = i(old DE)
MOV  H,B           ; HL = i  (from BC)
MOV  L,C
```

The intent is "HL = lhs, DE = rhs" but the allocator shuffles via BC because
`ensure_de(rhs)` and `ensure_hl(lhs)` conflict. Two fixes:

**a) Load order:** Load rhs first into DE, then load lhs into HL directly.
This avoids the collision entirely for the common case where both come from
memory:

```asm
LHLD _l_main_size   ; HL = size
XCHG                 ; DE = size
LHLD _l_main_i      ; HL = i
; compare HL vs DE
```

**b) Use XCHG:** When lhs is in DE and rhs is in HL, emit a single `XCHG`
(4 cycles) instead of the 4-MOV shuffle through BC (20 cycles).

---

## 4. Loop Induction Variable Kept in Memory (MEDIUM-HIGH)

Every loop iteration reloads and re-stores the loop counter:

```asm
; loop increment for i:
LHLD _l_main_i     ; reload i
INX  H
SHLD _l_main_i     ; store back
```

Because v6c uses global-mode allocation, `_l_main_i` is a static memory
location. The variable is loaded into HL at the top of the loop for the
comparison, incremented and stored at the bottom, then reloaded again at the
top of the next iteration.

**Proposed:** Register promotion for inner-loop induction variables. If a
variable is only used within a single loop body and is not address-taken, keep
it in a register pair (e.g. BC or DE) across the entire loop. Spill only at
loop exit.

For sieve.c's inner loop (`k += prime`):

```asm
; current (per iteration):
LHLD _l_main_k       ; 16 cycles
XCHG                  ; 4 cycles
LHLD _l_main_prime   ; 16 cycles
DAD  D                ; 10 cycles
SHLD _l_main_k       ; 16 cycles
; total: 62 cycles

; proposed (k in DE, prime in BC):
XCHG                  ; 4 cycles
DAD  D                ; 10 cycles (DE = updated k)
; total: 14 cycles
```

---

## 5. Comparison with Zero Uses Full Subtract Sequence (MEDIUM)

Several patterns compare a value against 0. Currently:

```asm
; flags[i] != 0 check:
MOV  A,H
ORA  L
JZ   L4__main
```

This part is already good (3 instructions). But the *initial* load of
H,L into the comparison path still goes through the full W16 compare machinery
when checking `== 0` or `!= 0`. At IR level, `if (x)` should be lowered
directly to `ORA L / JZ` without producing a comparison vreg at all.

**Proposed:** In IR generation or IR optimization, recognize:
- `JumpIfTrue(x, target)` where x is not a comparison → emit `ORA` test directly
- `Eq/Ne(x, 0)` followed by branch → fuse to `ORA L / JZ` or `ORA L / JNZ`

---

## 6. Identical Function Specializations Not Deduplicated (MEDIUM)

In `tmp_inline_asm.asm`, three copies of identical code exist:

```asm
add_asm:              ; original (never called)
    DAD D
    RET
__spec_add_asm_0:     ; specialization 0 (identical)
    DAD D
    RET
__spec_add_asm_1:     ; specialization 1 (identical)
    DAD D
    RET
```

**Proposed:** After specialization, run a deduplication pass: if two function
bodies are byte-identical, keep one and redirect all calls to it. Also
eliminate unreachable originals (the dead-function removal pass should already
handle this, but apparently the original `add_asm` survives).

---

## 7. Dead Code After Inline ASM Not Eliminated (MEDIUM)

In `tmp_nb.asm`:

```asm
; __asm_begin__
    CMA          ; result discarded
    INR A
; __asm_end__
    XRA A        ; clears A, overwriting asm result
    STA _g_result
```

The inline assembly computes a value in A, but the next instruction clears A.
The asm block is dead code.

**Root cause:** The compiler cannot analyze side-effects of inline assembly
blocks. Conservative assumption is correct, but a simple heuristic could help:
if an inline asm block has no outputs and no volatile annotation, and its only
effect is modifying registers that are immediately overwritten, it can be
eliminated.

---

## 8. Store-Reload Pattern Within Loop Body (MEDIUM)

In sieve.c after `count = count + 1`:

```asm
LHLD _g_count
INX  H
SHLD _g_count     ; store count
; ... loop continues ...
LHLD _g_count     ; reload count on next iteration
```

The value that was just placed in HL is stored, then immediately reloaded
at the next use. The load-store forwarding pass should eliminate this at the
IR level, but it seems to fail across loop back-edges.

**Proposed:** Extend load-store forwarding to work across basic block
boundaries within a loop, or implement a simple "available store" cache in
codegen that tracks the last `SHLD addr` and eliminates the following
`LHLD addr` when HL hasn't been clobbered.

The peephole already has Rule 1 (`SHLD addr / LHLD addr → SHLD addr`), but
there are typically intervening instructions between the store and reload.

---

## 9. W16 Boolean Result Stored in BC Then Immediately Tested (LOW-MEDIUM)

After every W16 comparison:

```asm
__cg_1:
    MOV  B,H      ; copy boolean to BC (never used again)
    MOV  C,L
    MOV  A,H      ; test HL directly
    ORA  L
    JZ   target
```

The `MOV B,H / MOV C,L` copy is completely dead — BC is never read afterwards.
This is an allocator artifact: the comparison result vreg is assigned to HL,
then the `JumpIfTrue` handler requests it via `ensure_hl(cond)`, but the
allocator first moves HL to BC to "save" it.

**Fix:** This is automatically resolved by implementing W16 compare-branch
fusion (item #1). Without fusion, a targeted fix would have the allocator
recognize that the vreg in HL is about to be consumed by the immediately
following ORA test and skip the save.

---

## 10. `LXI D,0` for Storing Zero to Array Element (LOW-MEDIUM)

In sieve.c:

```asm
; flags[k] = 0:
...
LXI  D,0
MOV  M,E        ; store low byte
INX  H
MOV  M,D        ; store high byte
```

Storing zero to an `int` could use:

```asm
MVI  M,0
INX  H
MVI  M,0
```

Or better, for arrays of int 0:

```asm
XRA  A
MOV  M,A
INX  H
MOV  M,A
```

This saves loading zero into a register pair. Similarly, storing 1:

```asm
MVI  M,1
INX  H
MVI  M,0
```

**Proposed:** In codegen, when StorePtr value is a known immediate, emit
`MVI M,n` directly instead of loading into DE first.

---

## 11. Missing Strength Reduction for `i + i + 3` (LOW-MEDIUM)

In sieve.c:

```c
prime = i + i + 3;
```

The IR generates two ADD operations. Since this is `2*i + 3`, it could be:

```asm
LHLD _l_main_i
DAD  H            ; HL = 2*i
INX  H            ; HL = 2*i + 1
INX  H            ; HL = 2*i + 2
INX  H            ; HL = 2*i + 3
```

Or with the `DAD` instruction:

```asm
LHLD _l_main_i
DAD  H            ; HL = 2*i
LXI  D,3
DAD  D            ; HL = 2*i + 3
```

Currently it loads `i` twice (once for each `Add` IR op).

---

## 12. Peephole: Missing W16 Compare-Branch Collapse (LOW)

The peephole optimizer has no rule to recognize the materialized boolean
pattern and collapse it into a direct branch. Even without codegen-level
fusion, a peephole rule could transform:

```asm
JM  __cg_0
JZ  __cg_0
LXI H,0
JMP __cg_1
__cg_0:
LXI H,1
__cg_1:
MOV B,H
MOV C,L
MOV A,H
ORA L
JZ  target
```

Into:

```asm
JM  skip
JZ  skip
JMP target
skip:
```

This is a safety-net optimisation in case codegen fusion cannot handle all
cases.

---

## 13. Inner Loops: Loop-Invariant Variable Reloaded Every Iteration (LOW)

In sieve.c, `_l_main_size` is loaded every iteration of the outer and inner
loops:

```asm
LHLD _l_main_size   ; loaded every iteration but never changes!
```

LICM (Loop Invariant Code Motion) should hoist this outside the loop. The
current LICM seems to not handle global loads (since they're conservatively
treated as having side-effects).

**Proposed:** Mark compiler-local variables (`_l_*`) as safe for LICM hoisting,
since they are known not to be aliased by external code.

---

## 14. `a[i] = a[i-1] + a[i]` Pattern Recomputes Array Base (LOW)

In dhrystone.c / fannkuch.c, expressions like `arr[i] = arr[i-1] + arr[i]`
generate three separate `__mul16` calls for computing `&arr[i]`, `&arr[i-1]`,
and `&arr[i]` again:

```asm
LHLD _l_proc_arr_i
LXI  D,2
CALL __mul16          ; compute &arr[i]
...
LHLD _l_proc_arr_i
DCX  H
...
LXI  D,2
CALL __mul16          ; compute &arr[i-1]
...
LHLD _l_proc_arr_i
...
LXI  D,2
CALL __mul16          ; compute &arr[i] again!
```

**Proposed:** CSE should recognize that `PtrAdd(arr, i, 2)` is the same
expression and reuse the result. The second computation of `&arr[i]` is a
missed CSE opportunity. Additionally, `&arr[i-1]` could be computed as
`&arr[i] - 2` (one `DCX` twice instead of a full multiply).

---

## 15. 8-bit Stores to Int Array Elements (LOW)

`flags[i] = 1` and `flags[k] = 0` store to an `int` (2-byte) array using:

```asm
LXI  D,1         ; 10 cycles
MOV  M,E         ; 7 cycles
INX  H           ; 5 cycles
MOV  M,D         ; 7 cycles
```

Since the values 0 and 1 fit in a byte and `int` on 8080 is 16-bit:

```asm
MVI  M,1         ; 10 cycles (low byte)
INX  H           ; 5 cycles
MVI  M,0         ; 10 cycles (high byte)
```

Both are similar in cycle count, but the `MVI M,n` form avoids occupying the
DE register pair, leaving it free for other values.

---

## Summary of Impact

| # | Optimization | Est. Savings | Difficulty |
|---|-------------|-------------|------------|
| 1 | W16 compare-branch fusion | ~35 cycles/compare | Medium |
| 2 | PtrAdd fast-path for element_size 2/4/8 | ~140 cycles/array access | Easy |
| 3 | Eliminate register shuffles around compares | ~16 cycles/compare | Medium |
| 4 | Loop induction variable register promotion | ~48 cycles/loop iter | Hard |
| 5 | Compare-with-zero fast path | ~10 cycles/test | Easy |
| 6 | Dedup identical specializations | Code size only | Easy |
| 7 | Dead inline asm elimination | Case-specific | Easy |
| 8 | Cross-block store-reload forwarding | ~32 cycles/reload | Medium |
| 9 | Eliminate dead BC copy after compare | ~8 cycles/compare | Easy (via #1) |
| 10 | MVI M,n for small immediates | ~5 cycles/store | Easy |
| 11 | Strength reduction for 2*i+k patterns | ~30 cycles/occurrence | Medium |
| 12 | Peephole compare-branch collapse | Safety net for #1 | Medium |
| 13 | LICM for _l_ variables in loops | ~16 cycles/loop iter | Medium |
| 14 | CSE for repeated PtrAdd | ~150 cycles/occurrence | Medium |
| 15 | MVI M,n for constant stores | ~0–5 cycles/store | Easy |

**Biggest wins:** Items 1, 2, and 4 together would dramatically improve code
quality on every non-trivial program. Item 2 alone (PtrAdd fast-path for
element_size=2) is a one-line code change with massive payoff.
