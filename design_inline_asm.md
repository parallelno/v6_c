# Inline Assembly Design — v6c Compiler

## Overview

Add `asm { ... }` block support to v6c, enabling hand-written Intel 8080
assembly within C functions.  Two forms:

1. **Full-body asm function** — entire function is raw asm. Zero overhead.
2. **Parameterized asm block** — inline asm statement with typed inputs/outputs.
   The compiler places C values into registers per the calling convention,
   emits the raw asm, and picks up the return value.  Only the declared
   registers are spilled/invalidated — untouched registers keep their
   compiler tracking.

Designed for maximum i8080 performance: zero prologue overhead for asm-only
functions, precise register-scoped spilling for inline blocks, and
self-modifying code support via the `* + 1` location-counter pattern.

---

## Syntax

### Grammar

```
asm_stmt := 'asm' '(' param_list ')' [ '->' type ] '{' raw_text '}'
          | 'asm' '{' raw_text '}'                         // raw mode, clobber-all
param_list := [ param_decl [ ',' param_decl ]* ]
param_decl := type c_variable_name
```

### Parameterized asm block (primary inline form)

The compiler treats `asm(params)` like an inline function call: it ensures
the named C variables are in the correct registers per calling convention,
emits the raw asm, and picks up the return value.

```c
void send_data(char *buf, int len) {
    for (int i = 0; i < len; i++) {
        char byte = buf[i];
        asm(char byte) {        // byte → A (8-bit arg0)
            OUT 42
        };
    }
}
```

With return value:
```c
int x = 10, y = 20;
int z = asm(int x, int y) -> int {  // x → HL, y → DE, result ← HL
    DAD D
};
```

No params, no return (side-effect only):
```c
asm() {
    EI              ; enable interrupts
};
```

### Raw asm block (clobber-all fallback)

When no parameter list is given, the compiler spills all live registers
before and invalidates everything after — safe but potentially costly.
Useful for large asm blocks that touch everything:

```c
int checksum(const char *data, int len) {
    int sum = 0;
    asm {
        LHLD _l_checksum_data
        XCHG
        LHLD _l_checksum_len
        ; ... full algorithm ...
        SHLD _l_checksum_sum
    }
    return sum;
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

### Mode 2 — Parameterized Asm Block

The `asm(params) { }` block appears among other C statements.  The compiler
places the named C variables into registers per the calling convention,
emits the raw asm, and picks up the return value.  **Only the declared
registers are spilled/invalidated.**

**Register mapping (same as calling convention):**

| Parameter | 16-bit | 8-bit |
|-----------|--------|-------|
| arg 0     | HL     | A     |
| arg 1     | DE     | —     |
| return    | HL     | A     |

**Compiler behaviour:**
1. Compute `touched = param_registers ∪ return_register`
2. Spill only registers in `touched` that hold live C values
3. Place input C variables into the correct registers
4. Emit the asm block text verbatim
5. Invalidate all registers in `touched` (except return, which gets
   fresh tracking for the result variable)
6. Registers **not** in `touched` keep their compiler tracking

**Examples:**

```c
// OUT a byte — only A touched, HL/DE/BC preserved
char val = 0xFF;
asm(char val) {
    OUT 42
};

// Add two ints — HL and DE touched, A/BC preserved
int z = asm(int x, int y) -> int {
    DAD D
};

// Enable interrupts — nothing touched, all regs preserved
asm() {
    EI
};

// Read memory — HL touched (input+output), others trust programmer
int val = asm(int ptr) -> int {
    MOV A, M
    INX H
    MOV H, M
    MOV L, A
};
```

**Register footprint summary:**

| Form                              | Regs touched | Spill scope |
|-----------------------------------|--------------|-------------|
| `asm { }`                         | all          | all         |
| `asm() { }`                       | none         | none        |
| `asm(char x) { }`                | A            | A only      |
| `asm(int a) -> int { }`          | HL           | HL only     |
| `asm(int a, int b) -> int { }`   | HL, DE       | HL, DE only |
| `asm(char x) -> char { }`        | A            | A only      |

**Note:** If the asm body internally uses registers beyond the declared
params/return (e.g. touches A when only HL is declared), that is the
programmer's responsibility — same trust model as full-body asm functions.

### Mode 2b — Raw Asm Block (clobber-all fallback)

The `asm { }` block without a parameter list is the raw/legacy form.
Useful for large blocks that touch many registers and reference `_l_`
labels directly.

**Compiler behaviour:**
1. **Flushes all live virtual registers** to their memory locations
2. Emits the asm block text verbatim
3. **Invalidates all register-tracking state** (assumes all clobbered)

**Example:**
```c
int checksum(const char *data, int len) {
    int sum = 0;
    asm {
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
        INX D
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

Inside **full-body asm functions** and **raw asm blocks**, lines matching
`identifier = expression` are passed through to the assembler as
EQU-style equates.  The compiler also parses these to adjust its own
behaviour:

| Equate Pattern             | Meaning                                      |
|----------------------------|----------------------------------------------|
| `_l_func_param = 0`       | Parameter stays in register; compiler skips memory allocation for it |
| `_l_func_param = * + 1`   | Parameter stored at instruction operand (self-modifying code) |
| `label = expr`            | General assembler equate (pass-through)      |

For **full-body asm functions**, the compiler scans equates whose names
match `_l_{func}_{param}` to decide which parameters need static-stack
allocation and which don't.  If a parameter label is declared `= 0`, no
memory is allocated.

*Note: parameterized asm blocks do not need equates — the compiler handles
register placement automatically.*

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
/// Inline assembly block.
AsmBlock {
    /// Raw assembly text between `{ }`.
    code: String,
    /// Typed input parameters: (C variable name, type).
    /// Empty for raw `asm { }` blocks.
    params: Vec<(String, CType)>,
    /// Return type, if `-> type` was specified.  `None` for void.
    return_type: Option<CType>,
},
```

### Phase 3 — Parser

**File:** `src/parser.rs`

In `parse_stmt()`, add a match arm:

```rust
TokenKind::Asm => self.parse_asm_block(),
```

New method `parse_asm_block()`:

```rust
fn parse_asm_block(&mut self) -> Option<Stmt> {
    let loc = self.advance().loc;  // consume `asm`

    // Parse optional parameter list: asm(int x, char y)
    let mut params = Vec::new();
    let mut return_type = None;
    let has_parens = self.check(&TokenKind::LParen);

    if has_parens {
        self.advance();  // consume '('
        while !self.check(&TokenKind::RParen) {
            let ty = self.parse_type_name()?;
            let name = self.expect_ident()?;
            params.push((name, ty));
            if !self.check(&TokenKind::RParen) {
                self.expect(&TokenKind::Comma)?;
            }
        }
        self.advance();  // consume ')'

        // Parse optional return type: -> int
        if self.check(&TokenKind::Arrow) {
            self.advance();  // consume '->'
            return_type = Some(self.parse_type_name()?);
        }
    }

    self.expect(&TokenKind::LBrace)?;
    let code = self.collect_raw_until_matching_brace();

    Some(Stmt::new(StmtKind::AsmBlock { code, params, return_type }, loc))
}
```

`collect_raw_until_matching_brace()` reads from the **original source
text** (not tokens) starting at the current position, counting brace
depth, and returns the raw string content.  This preserves assembly
formatting, labels, comments, and special characters that the C lexer
would choke on.

**Key:** The parser must switch to **raw-text mode** after `{`.  This
requires access to the source text and the byte offset of the current
token.  The `Token` struct already stores `loc` (line/col); we add the
byte offset so the parser can index into the source.

**Arrow token:** The `->` token may need to be added to the lexer if not
already present.  Alternatively, parse `Minus` + `Gt` as a two-token
sequence.

### Phase 4 — IR Generation

**File:** `src/ir_gen.rs`

Handle `StmtKind::AsmBlock`:

```rust
StmtKind::AsmBlock { code, params, return_type } => {
    // Resolve each param name to its vreg in the current scope.
    let input_vregs: Vec<(VReg, CType)> = params.iter().map(|(name, ty)| {
        let vreg = self.lookup_var(name);  // existing C variable
        (vreg, ty.clone())
    }).collect();
    let ret_ty = return_type.clone();
    self.emit(IrOp::InlineAsm {
        code: code.clone(),
        inputs: input_vregs,
        return_type: ret_ty,
    });
}
```

For **raw asm blocks** (no params), `inputs` is empty and `return_type`
is `None` — the codegen falls back to clobber-all.

**Full-body detection** — in `gen_func_def()`, check if the function body
is a single `Compound` containing a single `AsmBlock` **with empty params**.
If so:
- Set `func.is_asm_body = true` on the `IrFunction`
- Skip emitting parameter `StoreGlobal` prologue instructions
- Don't register parameters in `local_syms` (they won't be accessed by IR)
- Parse parameter equates from the asm code to determine which params
  need static-stack allocation

```rust
fn is_asm_only_body(body: &Stmt) -> bool {
    match &body.kind {
        StmtKind::AsmBlock { params, .. } if params.is_empty() => true,
        StmtKind::Compound(stmts) => {
            stmts.len() == 1 && matches!(
                stmts[0].kind,
                StmtKind::AsmBlock { ref params, .. } if params.is_empty()
            )
        }
        _ => false,
    }
}
```

### Phase 5 — IR

**File:** `src/ir.rs`

Add variant to `IrOp`:

```rust
/// Inline assembly block.
InlineAsm {
    /// Raw assembly text, emitted verbatim.
    code: String,
    /// Typed input vregs from the parameter list.
    /// Empty for raw `asm { }` blocks.
    inputs: Vec<(VReg, CType)>,
    /// Return type if `-> type` was specified.
    return_type: Option<CType>,
},
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

#### Parameterized asm block

In `gen_op()`, handle `IrOp::InlineAsm`:

```rust
IrOp::InlineAsm { code, inputs, return_type } => {
    if inputs.is_empty() && return_type.is_none() {
        // Raw mode: clobber all.
        self.regalloc.spill_all(&mut self.output);
        self.a_mirrors = None;
        for line in code.lines() {
            self.emit(line.to_string());
        }
        self.regalloc.invalidate_all();
    } else {
        // Parameterized mode: precise register tracking.
        // 1. Determine touched registers from params + return.
        let touched = compute_touched_regs(inputs, return_type);

        // 2. Spill only touched registers that hold live values.
        for &reg in &touched {
            if let Some(spill_op) = self.regalloc.spill(reg) {
                self.emit_move(&spill_op);
            }
        }
        self.a_mirrors = None;

        // 3. Place input values into correct registers.
        //    Reuses existing gen_call argument placement logic:
        //    arg0: 16-bit → HL, 8-bit → A
        //    arg1: 16-bit → DE
        for (i, (vreg, ty)) in inputs.iter().enumerate() {
            match (i, ty.size_of()) {
                (0, Some(1)) => self.ensure_a(*vreg),
                (0, _)       => self.ensure_hl(*vreg),
                (1, _)       => self.ensure_de(*vreg),
                _ => panic!("asm block supports max 2 params"),
            }
        }

        // 4. Emit raw assembly.
        for line in code.lines() {
            self.emit(line.to_string());
        }

        // 5. Invalidate touched regs; track return value.
        for &reg in &touched {
            self.regalloc.invalidate(reg);
        }
        if let Some(ret_ty) = return_type {
            let ret_reg = if ret_ty.size_of() == Some(1) {
                PhysReg::A
            } else {
                PhysReg::HL
            };
            // The destination vreg is assigned by IR gen;
            // mark it as living in ret_reg.
            self.mark(dst_vreg, ret_reg);
        }
    }
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
| `unknown variable 'x' in asm params` | IR gen: param name not in scope       |
| `asm block supports max 2 params`   | IR gen: >2 params exceeds registers   |
| `expected '->' or '{'`              | Parser: garbage after ')'             |
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
| `src/ast.rs`      | Add `StmtKind::AsmBlock { code, params, return_type }`|
| `src/parser.rs`   | `parse_asm_block()`, param list, `->`, raw-text       |
| `src/ir.rs`       | Add `IrOp::InlineAsm { inputs, return_type }`, `is_asm_body` |
| `src/ir_gen.rs`   | Handle `AsmBlock`, full-body detection, param resolve  |
| `src/callgraph.rs`| Skip param allocation for asm funcs, scan for CALLs   |
| `src/codegen.rs`  | Selective spill for params, full-body fast path        |

---

## Testing Strategy

Unit test files under `tests/unit/`:

1. **`asm_full_body.c`** — Full-body asm function (add, negate, identity)
2. **`asm_param.c`** — Parameterized asm blocks (OUT, add, read-memory)
3. **`asm_param_return.c`** — Asm blocks with `-> type` return values
4. **`asm_raw.c`** — Raw `asm { }` blocks using `_l_` labels
5. **`asm_strlib.c`** — Hand-optimized string functions (strlen, strcmp)
6. **`asm_selfmod.c`** — Self-modifying code patterns with `* + 1`
7. **`asm_loop.c`** — Asm block inside a loop (verify register preservation)

Verify by compiling to `.asm` and checking:
- Parameterized blocks only spill declared registers
- Full-body functions have no compiler prologue
- Return values are tracked correctly after asm blocks
- Raw blocks spill/invalidate everything
