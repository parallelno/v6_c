# Inline Asm — Clobber Hint Examples

## Syntax

```
asm_stmt := 'asm' [ '(' register_list ')' ] '{' raw_text '}'
register_list := register [ ',' register ]*
register := 'a' | 'bc' | 'de' | 'hl'
```

Default (no hint) = clobber all registers.

---

## Example 1 — Single register hint

```c
void game_loop(int score, int lives) {
    int x = score + 1;
    int y = lives;

    asm(hl) {
        LXI H, 42
        SHLD _l_game_loop_x
    }

    // compiler knows: only HL was clobbered
    // DE (if it held 'y') is still valid — no spill, no reload
    printf("%d %d", x, y);
}
```

## Example 2 — Multiple register hints

```c
void update(int *buf, int len) {
    int i = 0;

    asm(a, hl) {
        LHLD _l_update_buf
        MVI A, 0xFF
        MOV M, A
    }

    // only A and HL invalidated; DE (if it held 'len') survives

    asm(a) {
        MVI A, 7
        OUT 42
    }

    // only A invalidated; HL and DE survive
    return;
}
```

## Example 3 — No hint (default = clobber all)

```c
asm {
    ; conservative: compiler spills everything, invalidates everything
    LXI H, 42
    SHLD _l_foo_x
}
```

---

## Generated asm comparison

Given `HL=x(live), DE=y(live)` before the asm block:

| Syntax             | Spill              | Reload             | Overhead |
|--------------------|--------------------|--------------------|----------|
| `asm { ... }`      | SHLD+XCHG+SHLD    | LHLD+XCHG+LHLD    | **~72T** |
| `asm(hl) { ... }`  | SHLD (HL only)     | LHLD (HL only)     | **~32T** |
| `asm(a) { ... }`   | nothing            | nothing            | **~0T**  |

---

## OUT example — zero overhead with hint

```c
void main(void) {
    char val = 0xFF;
    asm(a) {
        LDA _l_main_val
        OUT 42
    }
    // A invalidated, but HL/DE/BC untouched
}
```

Output:
```asm
main:
    MVI A, 0xFF
    STA _l_main_val
    ; --- asm block (clobbers: A only) ---
    LDA _l_main_val
    OUT 42
    ; --- no spills, no reloads ---
    RET
```

---

## OUT example — full-body asm function (zero call overhead)

```c
void __global out_byte(char val) {
    asm {
        ; val arrives in A (8-bit arg0)
        OUT 42
    }
}
```

Output:
```asm
out_byte:
    OUT 42
    RET
```

---

# Parameterized Asm Blocks (Proposed)

The compiler treats `asm(params) { }` like an inline function call:
it places C variables into registers per calling convention, emits
the raw asm, and picks up the return value — all without labels or
manual spilling.

## Grammar

```
asm_stmt := 'asm' '(' param_list ')' [ '->' type ] '{' raw_text '}'
          | 'asm' '{' raw_text '}'                        // raw mode, clobber-all
param_list := param_decl [ ',' param_decl ]*
param_decl := type c_variable_name
```

## Register mapping (same as calling convention)

| Parameter | 16-bit | 8-bit |
|-----------|--------|-------|
| arg 0     | HL     | A     |
| arg 1     | DE     | —     |
| return    | HL     | A     |

---

## Example: OUT byte (zero overhead)

```c
char val = 0xFF;
asm(char val) {
    OUT 42
};
```

Compiler knows: A = input, no return → touched = {A}

Generated asm (assuming HL holds live C value):
```asm
    ; HL holds live value — NOT spilled (A-only footprint)
    MVI A, 0xFF          ; ensure val → A (may already be there)
    OUT 42
    ; HL still valid, DE still valid, only A invalidated
```

---

## Example: add two ints (return value)

```c
int x = 10;
int y = 20;
int z = asm(int x, int y) -> int {
    DAD D
};
// z = 30, in HL
```

Compiler knows: HL = input+output, DE = input → touched = {HL, DE}

Generated asm:
```asm
    ; compiler ensures x→HL, y→DE
    ; (if already there: zero cost)
    DAD D
    ; compiler tracks HL as 'z', DE invalidated
    ; BC, A untouched — preserved
```

---

## Example: read 16-bit from pointer

```c
int val = asm(int ptr) -> int {
    MOV A, M        ; low byte
    INX H
    MOV H, M        ; high byte
    MOV L, A
};
```

Compiler knows: HL = input+output → touched = {HL}
Note: A is used internally but not declared — programmer's responsibility.
Compiler trusts the param/return declaration (same as full-body asm).

---

## Example: increment char and return

```c
char x = 5;
char y = asm(char x) -> char {
    ADI 1
};
// y = 6, in A
```

Compiler knows: A = input+output → touched = {A}
HL, DE, BC all preserved.

---

## Example: side-effect only, no params

```c
asm() {
    EI              ; enable interrupts
};
```

Empty parens = no params, no return → touched = {} (nothing!)
All registers preserved. Equivalent to a no-op for register tracking.

---

## Example: I/O in a loop

```c
void send_data(char *buf, int len) {
    for (int i = 0; i < len; i++) {
        char byte = buf[i];
        asm(char byte) {
            OUT 42
        };
    }
}
```

Compiler knows each iteration: A = input → touched = {A}
Loop index in HL/DE stays valid across asm blocks — no spill/reload.

---

## Register footprint summary

| Form                              | Regs touched | Spill scope |
|-----------------------------------|--------------|-------------|
| `asm { }`                         | all          | all         |
| `asm() { }`                       | none         | none        |
| `asm(char x) { }`                | A            | A only      |
| `asm(int a) -> int { }`          | HL           | HL only     |
| `asm(int a, int b) -> int { }`   | HL, DE       | HL, DE only |
| `asm(char x) -> char { }`        | A            | A only      |

Untouched registers keep their compiler tracking — no spill, no reload,
no overhead.
