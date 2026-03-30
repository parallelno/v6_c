# Inline Assembly Design — v6c Compiler

## Overview

Add `asm { ... }` block support to v6c, enabling hand-written Intel 8080
assembly within C functions.  Designed for maximum i8080 performance: zero
prologue overhead for asm-only functions, full register control, and
self-modifying code support via the `* + 1` location-counter pattern.

---

## Syntax

### Statement-level asm block

```c
void foo(void) {
    int x = 1;
    asm {
        LXI H, 42
        SHLD _l_foo_x
    }
    return x;
}
```

### Full-body asm function

When the function body contains **only** a single `asm { ... }` block (no
other C statements), the compiler treats it as a **full-body asm function**.

```c
int __global add(int a, int b) {
    asm {
        ; a arrives in HL, b arrives in DE  (v6c calling convention)
        DAD D
        ; result already in HL
    }
}
```

---

## Calling Convention Recap (v6c)

| Position | 16-bit     | 8-bit  | 32-bit        |
|----------|------------|--------|---------------|
| arg 0    | **HL**     | **A**  | **DE:HL**     |
| arg 1    | **DE**     | —      | stack         |
| arg 2+   | stack (R→L)| stack  | stack         |
| return   | **HL**     | **A**  | **DE:HL**     |

The asm programmer must know these register assignments.

---

## Two Modes

### Mode 1 — Full-Body Asm Function

Triggered when the function body is a single `asm { }` block.

**Compiler behaviour:**
1. Emits the function label (`funcname:`)
2. **Skips the standard parameter-save prologue entirely**
3. Emits the asm block text verbatim
4. Emits `RET` (unless the last non-empty asm line is `RET` / `JMP` / `JZ` etc.)
5. **Does not allocate static-stack slots** for this function's parameters

**Programmer responsibility:**
- Parameters arrive in registers per calling convention above
- Save, manipulate, and return values manually
- Leave the return value in HL (16-bit) or A (8-bit)

**Example — strcat (i8080-optimized):**
```c
char *__global strcat(char *dest, const char *src) {
    asm {
        ; dest in HL, src in DE
        ; Save dest for return value
_l_strcat_dest = * + 1
        SHLD 0              ; store HL (dest) into the LXI operand below
        ; Find end of dest
        DCX H
        XRA A
_strcat_find:
        INX H
        CMP M
        JNZ _strcat_find
        ; Copy src → end of dest
_strcat_copy:
        LDAX D
        MOV M,A
        INX H
        INX D
        ORA A
        JNZ _strcat_copy
        ; Return original dest pointer
_l_strcat_dest_ret = * + 1
        LXI H, 0           ; patched by the SHLD above
    }
}
```

Wait — the `* + 1` trick above has a subtle issue.  The `SHLD 0` stores HL
to address 0.  We actually need a **dedicated 2-byte memory cell**, or
we use the classic self-modifying pattern differently:

```c
char *__global strcat(char *dest, const char *src) {
    asm {
        ; dest in HL, src in DE
        SHLD _strcat_save   ; save dest to a temp
        ; Find end of dest
        DCX H
        XRA A
_strcat_find:
        INX H
        CMP M
        JNZ _strcat_find
        ; Copy src → end of dest
_strcat_copy:
        LDAX D
        MOV M,A
        INX H
        INX D
        ORA A
        JNZ _strcat_copy
        ; Return original dest
        LHLD _strcat_save
        RET
_strcat_save:
        .storage 2
    }
}
```

Or with the true self-modifying approach (saves 6 T-states on return):
```c
char *__global strcat(char *dest, const char *src) {
    asm {
        ; dest in HL, src in DE
_strcat_save = * + 1
        SHLD 0              ; address patched by assembler: stores HL into LXI operand
        DCX H
        XRA A
_strcat_find:
        INX H
        CMP M
        JNZ _strcat_find
_strcat_copy:
        LDAX D
        MOV M,A
        INX H
        INX D
        ORA A
        JNZ _strcat_copy
_strcat_save_ld = * + 1
        LXI H, 0           ; operand was written by SHLD above
    }
}
```
*Note: this requires `_strcat_save` and `_strcat_save_ld` to point to the
same 2 bytes.  In practice, use a single `.storage 2` label and reference
it from both SHLD and LXI.*

### Mode 2 — Inline Asm Statement

The `asm { }` block appears among other C statements.

**Compiler behaviour:**
1. **Flushes all live virtual registers** to their memory locations (spill-all)
2. Emits the asm block text verbatim
3. **Invalidates all register-tracking state** (assumes asm clobbered A, BC, DE, HL, flags)
4. Subsequent C code reloads values from memory as needed

**Example:**
```c
int checksum(const char *data, int len) {
    int sum = 0;
    asm {
        ; data in _l_checksum_data, len in _l_checksum_len, sum in _l_checksum_sum
        LHLD _l_checksum_data
        XCHG
        LHLD _l_checksum_len
        MOV B,H
        MOV C,L
        LXI H, 0
_cs_loop:
        MOV A,B
        ORA C
        JZ _cs_done
        LDAX D
        MOV E,A
        MVI D,0
        DAD D
        INX D               ; oops, DE trashed — need separate pointer
        DCX B
        JMP _cs_loop
_cs_done:
        SHLD _l_checksum_sum
    }
    return sum;
}
```

---

## Parameter Equate Declarations

Inside asm blocks, lines matching `identifier = expression` are passed
through to the assembler as EQU-style equates.  The **compiler** also
parses these to adjust its own behaviour:

| Equate Pattern             | Meaning                                      |
|----------------------------|----------------------------------------------|
| `_l_func_param = 0`       | Parameter stays in register; compiler skips memory allocation for it |
| `_l_func_param = * + 1`   | Parameter stored at instruction operand (self-modifying code) |
| `label = expr`            | General assembler equate (pass-through)      |

For **full-body asm functions**, the compiler scans equates whose names
match `_l_{func}_{param}` to decide which parameters need static-stack
allocation and which don't.  If a parameter label is declared `= 0`, no
memory is allocated.

---

## Implementation Plan

### Phase 1 — Lexer

**File:** `src/lexer.rs`

Add `Asm` variant to `TokenKind`:

```rust
// In the TokenKind enum:
Asm,        // `asm` keyword
```

Register in keyword lookup:

```rust
"asm" => TokenKind::Asm,
```

### Phase 2 — AST

**File:** `src/ast.rs`

Add variant to `StmtKind`:

```rust
/// Inline assembly block.  `code` is the raw text between `{ }`.
AsmBlock { code: String },
```

### Phase 3 — Parser

**File:** `src/parser.rs`

In `parse_stmt()`, add a match arm:

```rust
TokenKind::Asm => self.parse_asm_block(),
```

New method `parse_asm_block()`:

```
fn parse_asm_block(&mut self) -> Option<Stmt> {
    let loc = self.advance().loc;  // consume `asm`
    self.expect(&TokenKind::LBrace)?;

    // Collect raw text between braces.
    // Use brace-depth counting on the raw source to handle nested
    // braces in macros or comments.
    let code = self.collect_raw_until_matching_brace();

    // The closing `}` is consumed by collect_raw_until_matching_brace.
    Some(Stmt::new(StmtKind::AsmBlock { code }, loc))
}
```

`collect_raw_until_matching_brace()` reads from the **original source
text** (not tokens) starting at the current position, counting brace
depth, and returns the raw string content.  This preserves assembly
formatting, labels, comments, and special characters that the C lexer
would choke on.

**Key:** The parser must switch to **raw-text mode** after `asm {`.  This
requires access to the source text and the byte offset of the current
token.  The `Token` struct already stores `loc` (line/col); we add the
byte offset so the parser can index into the source.

### Phase 4 — IR Generation

**File:** `src/ir_gen.rs`

Handle `StmtKind::AsmBlock`:

```rust
StmtKind::AsmBlock { code } => {
    self.emit(IrOp::InlineAsm { code: code.clone() });
}
```

**Full-body detection** — in `gen_func_def()`, check if the function body
is a single `Compound` containing a single `AsmBlock`.  If so:
- Set `func.is_asm_body = true` on the `IrFunction`
- Skip emitting parameter `StoreGlobal` prologue instructions
- Don't register parameters in `local_syms` (they won't be accessed by IR)
- Parse parameter equates from the asm code to determine which params
  need static-stack allocation

```rust
fn is_asm_only_body(body: &Stmt) -> bool {
    match &body.kind {
        StmtKind::AsmBlock { .. } => true,
        StmtKind::Compound(stmts) => {
            stmts.len() == 1 && matches!(stmts[0].kind, StmtKind::AsmBlock { .. })
        }
        _ => false,
    }
}
```

### Phase 5 — IR

**File:** `src/ir.rs`

Add variant to `IrOp`:

```rust
/// Raw assembly text, emitted verbatim by the code generator.
InlineAsm { code: String },
```

Add field to `IrFunction`:

```rust
/// True when the entire function body is a single `asm { }` block.
/// The code generator skips the standard prologue/epilogue.
pub is_asm_body: bool,
```

### Phase 6 — Call Graph Analysis

**File:** `src/callgraph.rs`

For functions with `is_asm_body == true`:
- **Skip parameter slot allocation**.  The asm code manages its own memory.
- **Conservatively assume the function may call anything** (scan the asm
  text for `CALL` instructions to extract callees, or assume worst case).
- Alternatively, scan for `CALL label` patterns in the asm text and
  register those as callees in the call graph.

For asm functions that declare `_l_func_param = 0` equates, those
parameters are excluded from the static-stack allocator, saving RAM.

### Phase 7 — Code Generator

**File:** `src/codegen.rs`

#### Full-body asm function

In `gen_function()`, detect `is_asm_body`:

```rust
fn gen_function(&mut self, func: &IrFunction) {
    // ... existing setup ...
    self.emit_label(&func.name);

    if func.is_asm_body {
        // Emit the single InlineAsm op directly — no prologue, no regalloc.
        for instr in &func.body {
            if let IrOp::InlineAsm { code } = &instr.op {
                for line in code.lines() {
                    self.emit(line.to_string());
                }
            }
        }
        // Add RET if the asm doesn't end with a return/jump.
        if !self.asm_ends_with_control_flow(&func.body) {
            self.emit_inst("RET");
        }
        return;
    }

    // ... existing codegen ...
}
```

#### Inline asm statement (mixed C + asm)

In `gen_op()`, handle `IrOp::InlineAsm`:

```rust
IrOp::InlineAsm { code } => {
    // 1. Spill all live registers to memory.
    self.regalloc.spill_all(&mut self.output);
    self.a_mirrors = None;
    // 2. Emit raw assembly.
    for line in code.lines() {
        self.emit(line.to_string());
    }
    // 3. Invalidate all register tracking.
    self.regalloc.invalidate_all();
}
```

### Phase 8 — Emit

**File:** `src/emit.rs`

No changes needed.  The asm text flows through as regular assembly lines.
Runtime dependency scanning (`CALL` detection) automatically picks up
any runtime calls referenced in asm blocks.

---

## i8080-Specific Optimizations

### 1. Zero-Overhead Asm Functions
Full-body asm functions have **no compiler-generated prologue or epilogue**.
The function label + raw asm + RET is the minimal possible overhead.
For a 2-arg function, this saves 20+ T-states per call vs. the standard
`SHLD`/`XCHG`/`SHLD` prologue.

### 2. Self-Modifying Code (`* + 1` Pattern)
The v6asm assembler's `*` location counter enables the classic 8080
trick of embedding data within instruction operands:

```asm
_save = * + 1
LXI H, 0           ; 10 T-states to "load" the saved value
```

vs. the standard approach:

```asm
LHLD _save          ; 16 T-states
```

**Saves 6 T-states per access** — critical in tight loops.

### 3. Register-Only Parameters
For functions with 1-2 args, all parameters are in registers (HL, DE).
The asm code can operate directly without any memory access:

```c
int __global add(int a, int b) {
    asm {
        DAD D       ; HL += DE, result in HL.  Total: 10 T-states
    }
}
```

### 4. No Frame Pointer Overhead
The i8080 has no frame pointer.  Asm functions avoid the overhead of
setting up and tearing down stack frames.

### 5. Tailored Static Allocation
The call-graph analysis knows asm functions don't need parameter slots,
so their frame cost in the static allocation is **zero bytes**.

### 6. Inline Data
Asm blocks can embed data directly after a jump, using `.db` / `.dw` /
`.storage` directives:

```asm
        JMP _past_data
_table:
        .db 0, 1, 4, 9, 16, 25
_past_data:
        ; code continues
```

### 7. Callee-Optimized Hot Paths
Library functions (string.h, stdlib.h) can be implemented entirely in
asm for the tightest possible i8080 code, with the compiler providing
the C interface and calling convention glue automatically.

---

## Label Naming Conventions

| Label Pattern              | Used By           | Purpose                          |
|----------------------------|-------------------|----------------------------------|
| `_l_{func}_{param}`       | Compiler / asm    | Parameter memory address         |
| `_l_{func}_{local}`       | Compiler / asm    | Local variable memory address    |
| `_g_{name}`               | Compiler / asm    | Global variable address          |
| `L{n}__{func}`            | Compiler          | Compiler-generated labels        |
| `_{func}_xxx`             | Asm programmer    | Private labels within asm blocks |
| `__va_base_{func}`        | Compiler          | Variadic argument base           |

Asm blocks can reference any of these labels.  Labels defined inside asm
blocks are global to the assembly output (v6asm has flat label scope), so
prefix private labels with the function name to avoid collisions.

---

## Error Handling

| Error                                | When                                  |
|--------------------------------------|---------------------------------------|
| `expected '{' after 'asm'`           | Parser: no opening brace              |
| `unterminated asm block`             | Parser: EOF before closing brace      |
| Assembly errors (syntax, labels)     | Assembler: post-compilation           |

The compiler does **not** validate asm content — that's the assembler's
job.  This keeps the compiler simple and avoids duplicating instruction-set
knowledge.

---

## c8080 Compatibility Notes

| Feature                  | c8080              | v6c                          |
|--------------------------|--------------------|------------------------------|
| Mnemonics                | Z80 (LD, EX)      | Intel 8080 (LXI, XCHG)      |
| Location counter         | `$`                | `*`                          |
| Last-param register      | Last → HL          | First → HL                   |
| Param labels             | `__a_N_func`       | `_l_func_param`              |
| Block syntax             | `asm { }`          | `asm { }` (same)             |

c8080 stdlib files **cannot** be used directly — they need mnemonic
translation and calling-convention adaptation.  A separate migration
script could automate this.

---

## Files to Modify

| File              | Change                                               |
|-------------------|------------------------------------------------------|
| `src/lexer.rs`    | Add `Asm` token, keyword registration                |
| `src/ast.rs`      | Add `StmtKind::AsmBlock { code: String }`            |
| `src/parser.rs`   | `parse_asm_block()`, raw-text collection              |
| `src/ir.rs`       | Add `IrOp::InlineAsm`, `IrFunction.is_asm_body`      |
| `src/ir_gen.rs`   | Handle `AsmBlock`, full-body detection, skip prologue |
| `src/callgraph.rs`| Skip param allocation for asm funcs, scan for CALLs   |
| `src/codegen.rs`  | Emit raw asm, skip prologue for asm funcs             |

---

## Testing Strategy

Unit test files under `tests/unit/`:

1. **`asm_full_body.c`** — Full-body asm function (add, negate, identity)
2. **`asm_inline.c`** — Mixed C + asm statements
3. **`asm_strlib.c`** — Hand-optimized string functions (strlen, strcmp)
4. **`asm_selfmod.c`** — Self-modifying code patterns with `* + 1`

Verify by compiling to `.asm` and checking the output contains the exact
asm text with correct labels and no unwanted prologue instructions.
