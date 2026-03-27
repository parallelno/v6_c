# Plan: C Compiler Written in Rust Targeting the Intel 8080

## 1. Audit of Existing C‑to‑8080/Z80 Compilers

### 1.1 c8080 (alemorf/c8080)

**Overview.** c8080 is an open‑source C compiler written in C++ that directly targets the Intel 8080. It includes a C parser, an expression‑tree IR, tree‑level optimizations, and an 8080 code generator.
The name of the compiler executable file is v6c.

**Key performance‑relevant design choices:**

| Feature | Detail |
|---|---|
| **Static allocation mode (`__global`)** | Default mode. The compiler resolves the entire call graph at compile time and places every function's locals and parameters at **fixed RAM addresses**. No stack frame setup/teardown at runtime. A function like `int calc(int a, int b)` compiles to 3 instructions instead of ≈10. **Tradeoff:** recursion is forbidden for `__global` functions. |
| **Stack mode (`__stack`)** | Traditional SP‑relative addressing. Generates `LD HL,offset / ADD HL,SP / LD r,(HL)` sequences — large and slow on the 8080. Opt‑in per function. |
| **Register parameter passing** | The last parameter of a fixed‑arity function is passed in registers: `A` (8‑bit), `HL` (16‑bit), or `DE:HL` (32‑bit). Avoids a memory store/load round‑trip. |
| **Inline assembly** | `asm { … }` blocks with access to compiler‑generated parameter labels (`__a_N_funcname`). Allows hand‑tuning hot paths and even self‑modifying code (parameter embedded in an immediate operand). |
| **Standard library in assembly** | Headers ship as C prototypes backed by assembly implementations (string.h, stdio.h, stdlib.h, etc.), bundled with sjasmplus. |

**Performance assessment.**
The `__global` static‑allocation model is the single biggest performance win for 8080 targets. On an 8080 there is **no frame pointer register** and computing `SP + offset` costs 20+ cycles per access. Eliminating that overhead makes compiled C competitive with hand‑written assembly for non‑recursive code. The register‑passing convention adds a smaller but still worthwhile improvement.

**Limitations to learn from** (addressed in later phases of our plan):
* No IR‑level optimization passes beyond tree simplification — no CSE, no dead‑code elimination, no loop transformations. → *Addressed in Phase 2 (§3) and Phase 5.*
* No peephole optimizer on emitted assembly. → *Addressed in Phase 1 (step 1.9) and Phase 2 (step 2.6).*
* No register allocator — the 8080's scarce register set is managed by fixed templates. → *Addressed in Phase 5 (step 5.1).*
* The `__global` mode requires whole‑program compilation (all call paths known); separate compilation or function pointers break the model.

---

### 1.2 z88dk Benchmark Suite (Z80 compilers)

The z88dk project publishes the most thorough benchmark comparison of Z80 C compilers. Although Z80 is a superset of 8080, the results are directly relevant: every Z80 compiler must still use the 8080 base instruction set for core integer work, and the 8080 lacks the Z80 index registers (IX/IY) that some compilers rely on for stack frames.

**Compilers tested:** Hitech‑C CP/M v3.09, Hitech‑C Z80 v7.80, IAR Z80 V4.06A, SDCC 4.2.0, z88dk/sccz80 (classic & new libs), z88dk/sdcc (classic & new libs).

#### Benchmark Summary

| Benchmark | Best Speed | Best Size | Key Insight |
|---|---|---|---|
| **Dhrystone 2.1** (integer, synthetic) | SDCC — 354.73 Dhrystones/s | SDCC — 6825 B | Code generator quality dominates; register calling convention (`__sdcccall(1)`) helps SDCC. |
| **Sieve of Eratosthenes** (loop overhead) | Z88DK/SDCC_NEW — 0.92 s | SDCC — 8278 B | Tight loops magnify per‑iteration overhead; sccz80's subroutine‑call primitives hurt here. |
| **Fannkuch** (array manipulation) | Hitech‑C v7.80 — 13.0 s | Hitech‑C v7.80 — 868 B | Classic optimizing compiler wins both axes; strong local optimization matters. |
| **Pi** (32‑bit integer math) | Z88DK/SCCZ80_NEW_FAST — 7 min (no ldiv) / **5 min 25 s** (ldiv) | Z88DK/SDCC_NEW — 6246 B | **Assembly 32‑bit math library** is the decisive factor; Z88DK's fast‑int lib is 3–4× faster than C‑based libs. |
| **Fasta** (float + random) | Z88DK/SCCZ80/MATH32 — 34.0 s | Z88DK/SCCZ80_NEW — 2998 B | Assembly float library (IEEE‑754 math32) crushes C‑implemented floats (SDCC: 93 s). |
| **n‑Body** (float‑heavy physics) | Z88DK/SCCZ80_NEW/MATH32 — 3 min 8 s | Z88DK/SCCZ80_NEW — 3363 B | Same story — assembly math32 package is 4× faster than the next best. |
| **Whetstone 1.2** (float synthetic) | Z88DK/SCCZ80/MATH32 — 7.05 KWIPS | Z88DK/SCCZ80_NEW — 5362 B | Assembly float wins again; 32‑bit mantissa is sufficient and far faster than 48‑bit. |
| **Binary‑Trees** (malloc/free stress) | Z88DK/SCCZ80_CLASSIC — 36.4 s | Z88DK/SDCC_NEW — 2689 B | Heap allocator implementation quality matters more than the compiler. |

#### Cross‑Cutting Findings

1. **Assembly runtime libraries are the #1 lever.** For math‑heavy code, switching from a C‑implemented library to an assembly one yields 2–4× speedups — more than any compiler optimization.
2. **Register‑based calling conventions help.** SDCC's `__sdcccall(1)` and z88dk's `__z88dk_fastcall` / `__z88dk_callee` consistently outperform plain stack‑only conventions.
3. **Subroutine‑call primitives are a trap for tight loops.** sccz80 saves code size by turning primitive operations (shifts, comparisons) into CALL instructions, but the CALL/RET overhead (17+10 = 27 cycles) is devastating in inner loops (Sieve benchmark).
4. **A good peephole optimizer closes the gap.** Hitech‑C v7.80's global optimizer makes it competitive despite being decades old — pattern‑based cleanup of redundant loads, stores, and jumps is high‑value, low‑complexity.
5. **Whole‑program analysis unlocks 8080‑specific wins.** The 8080 has only 7 general registers (A B C D E H L) and no index registers. Knowing the full call graph allows static allocation (c8080), global register assignment, and dead‑function elimination.

---

## 2. Compiler Design

### 2.1 Goals

| Priority | Goal |
|---|---|
| **P0** | Produce the fastest possible 8080 machine code for the supported C subset. |
| **P1** | Keep the architecture modular so additional C features can be added incrementally. |
| **P2** | Emit readable, debuggable assembly. |
| **P3** | Keep the compiler itself simple and maintainable (written in Rust). |

### 2.2 High‑Level Architecture

```
  ┌───────────┐     ┌───────────┐     ┌───────────┐     ┌───────────┐     ┌───────────┐
  │  Lexer    │────▶│  Parser   │────▶│    IR     │────▶│  Code Gen │────▶│  Peephole │──▶ .asm
  │  (tokens) │     │  (AST)    │     │  (3-addr) │     │  (8080)   │     │  Optimizer│
  └───────────┘     └───────────┘     └───────────┘     └───────────┘     └───────────┘
                                            │
                                      ┌─────┴─────┐
                                      │ IR Passes  │
                                      │ (optimize) │
                                      └───────────┘
```

The compiler is a single‑pass‑per‑stage pipeline:

1. **Lexer** → token stream.
2. **Parser** → AST (abstract syntax tree).
3. **AST → IR lowering** → three‑address‑code IR with virtual registers.
4. **IR optimization passes** → constant folding/propagation, dead‑code elimination, strength reduction, common sub‑expression elimination.
5. **Code generator** → Intel 8080 assembly with physical register allocation.
6. **Peephole optimizer** → pattern‑matched rewriting of emitted assembly.
7. **Output** → assembly text for the **v6asm** assembler (https://github.com/parallelno/v6_assembler), targeting the Vector 06 computer (starting address `0x100`).

### 2.3 Calling Convention & Memory Model

Drawing on the audit findings, the compiler uses a **dual‑mode** calling convention:

#### Default: Static Allocation (`global` mode)

* The compiler performs whole‑program call‑graph analysis.
* Every non‑recursive function's locals and parameters are assigned **fixed memory addresses** at compile time.
* Function arguments are written directly to their assigned addresses by the caller (or passed in registers — see below).
* **Result:** zero stack‑frame overhead; variable access is a single `LDA addr` / `STA addr` or `LHLD addr` / `SHLD addr`.

#### Opt‑in: Stack Allocation (`stack` mode)

* For functions marked `__stack` (or automatically for recursive/indirectly‑called functions).
* Uses a software frame pointer in a dedicated register pair (DE by default) or SP‑relative addressing via helper routines.
* Required for recursion, function pointers, and reentrant code.

#### Register Passing

* The **first 16‑bit argument** (or return value) goes in **HL**.
* The **second 16‑bit argument** goes in **DE**.
* An 8‑bit argument goes in **A**.
* Additional arguments go to their static addresses (global mode) or are pushed to the stack (stack mode).
* Return value: **HL** (16‑bit), **A** (8‑bit), **DE:HL** (32‑bit).

### 2.4 Initial C Subset (Phase 1)

The minimal subset sufficient for useful programs and benchmarking:

| Category | Supported |
|---|---|
| **Types** | `char` (8‑bit), `int` (16‑bit), `long` (32‑bit), `unsigned` variants, pointers, arrays, `void` |
| **Operators** | Full set: arithmetic, bitwise, logical, comparison, assignment, compound assignment, `sizeof`, cast, address‑of, dereference |
| **Control flow** | `if`/`else`, `while`, `for`, `do`/`while`, `break`, `continue`, `return`, `goto` |
| **Functions** | Definitions, declarations, calls (no variadic in Phase 1) |
| **Variables** | Global, local, `static`, `const`, `extern` |
| **Preprocessor** | `#include`, `#define` (object‑like and function‑like), `#ifdef`/`#ifndef`/`#endif`, `#if`/`#elif`/`#else`, `#undef`, `#error` |
| **Other** | Single‑line and multi‑line comments, string literals, character literals, integer constants (decimal, hex, octal) |

**Not in Phase 1** (deferred): `struct`/`union`, `enum`, `typedef`, variadic functions, floating point, `switch`/`case`, bit‑fields, initializer lists, multi‑dimensional arrays.

### 2.5 IR Design

A **three‑address code** (TAC) IR with virtual registers:

```
t1 = LOAD_GLOBAL  addr        // load from fixed address
t2 = ADD  t1, t3              // binary op
     STORE_GLOBAL addr, t2    // store to fixed address
     IF_FALSE t2, label       // conditional branch
     CALL func, t1, t2        // function call
t3 = CALL_RESULT              // retrieve return value
```

Virtual registers are unlimited; the register allocator maps them to the 8080's physical registers (A, B, C, D, E, H, L, plus the stack for spills).

**Key IR properties:**
* Explicit loads/stores (no implicit memory operands) — enables load/store optimization.
* Separate opcodes for 8‑bit vs 16‑bit vs 32‑bit operations — the code generator selects the optimal instruction sequence for each width.
* SSA form is not required initially but the IR is structured to allow SSA conversion later.

### 2.6 Code Generator Strategy

The code generator translates each IR instruction to an 8080 instruction sequence. Key strategies:

#### 2.6.1 Pattern Matching

Instead of one‑instruction‑at‑a‑time translation, the code generator recognizes **multi‑instruction IR patterns** and emits specialized sequences:

| IR Pattern | Optimized 8080 Output |
|---|---|
| `t = a + 1` (16‑bit) | `LHLD a / INX H / SHLD t` |
| `t = a + const` (16‑bit, small const) | `LHLD a / LXI D,const / DAD D / SHLD t` |
| `t = arr[i]` (byte array) | `LHLD i / LXI D,arr / DAD D / MOV A,M` |
| `t = arr[i]` (word array) | `LHLD i / DAD H / LXI D,arr / DAD D / MOV E,M / INX H / MOV D,M` |
| `if (a == 0) goto L` | `LHLD a / MOV A,H / ORA L / JZ L` |
| `if (a < b)` unsigned 16‑bit | Compare with helper — or inline subtract + carry check |

#### 2.6.2 Register Allocation

Given the 8080's extreme register scarcity (7 registers, only HL usable for indirect addressing), a full graph‑coloring allocator is overkill. Instead:

1. **HL is the primary accumulator** for 16‑bit values and pointer dereferences.
2. **DE is the secondary pair** for binary operations (`DAD D`).
3. **BC** is used for loop counters and tertiary storage.
4. **A** is used for 8‑bit operations and comparisons.
5. Spills go to **fixed memory locations** (in global mode, these are already the variable addresses; no extra cost).

The allocator is a **linear scan** over basic blocks, preferring to keep the most‑used value in HL.

#### 2.6.3 Strength Reduction (target‑aware)

| C Operation | 8080 Strength Reduction |
|---|---|
| `x * 2` | `DAD H` (add HL to itself) — 10 cycles vs multiply routine |
| `x * 3` | `DAD H; DAD D` (with DE = original x) |
| `x * 2^n` | Chain of `DAD H` (up to n ≈ 4, then shift loop) |
| `x / 2` (unsigned) | `MOV A,H / ORA A / RAR / MOV H,A / MOV A,L / RAR / MOV L,A` |
| `x % 256` | `MOV H,0` (mask high byte) |
| `x << 1` (8‑bit) | `ADD A` (self‑add is fastest left shift) |

### 2.7 Peephole Optimizer

A pattern‑matching pass over the emitted assembly text. Rules are expressed as (match → replace) pairs:

```
// Remove redundant load after store
SHLD addr           SHLD addr
LHLD addr       →   (deleted)

// Remove redundant move
MOV A,B             (deleted if A is already B)
MOV B,A

// Simplify zero-test
MOV A,H             MOV A,H
ORA L               ORA L
CPI 0           →   (deleted — ORA already sets Z)
JZ label            JZ label

// Tail-call optimization
CALL func           JMP func
RET              →   (deleted)

```

The peephole runs in a loop until no more rules fire (fixed‑point).

### 2.8 Assembly Runtime Library

Based on the z88dk benchmark findings, an **assembly‑language runtime library** is critical. Phase 1 covers:

| Module | Functions |
|---|---|
| **16‑bit math** | `__mul16`, `__div16u`, `__div16s`, `__mod16u`, `__mod16s` |
| **32‑bit math** | `__mul32`, `__div32u`, `__div32s`, `__mod32u`, `__mod32s`, `__shl32`, `__shr32u`, `__shr32s` |
| **16‑bit shifts** | `__shl16`, `__shr16u` (logical), `__shr16s` (arithmetic) |
| **Comparison** | `__cmp16s`, `__cmp16u`, `__cmp32s`, `__cmp32u` |
| **Memory** | `memcpy`, `memset`, `memmove`, `strlen`, `strcmp` |
| **I/O** | `putchar`, `getchar` (target‑specific stubs) |

Each routine is hand‑optimized for the 8080 instruction set. The multiply/divide routines use the shift‑and‑add/subtract algorithms tuned for minimal cycle count (unrolled where beneficial).

---

## 3. Implementation Phases

### Phase 1 — Minimum Viable Compiler

**Goal:** Compile a single‑file C program (Sieve of Eratosthenes) to correct, runnable 8080 code.

- [x] **1.1 Lexer** — Tokenize C source: keywords, identifiers, integer constants, string literals, operators, punctuation.
- [x] **1.2 Preprocessor** — `#include`, `#define`, `#ifdef`/`#ifndef`/`#endif`, `#if`/`#elif`/`#else`.
- [x] **1.3 Parser** — Recursive‑descent parser producing an AST. Support: functions, global/local variables, `if`/`else`, `while`, `for`, `return`, basic expressions.
- [x] **1.4 Type system** — `char`, `int`, `long`, `unsigned` variants, pointers. Implicit widening. Cast operator.
- [x] **1.5 AST → IR lowering** — Translate AST to three‑address code with virtual registers.
- [x] **1.6 Call‑graph analysis** — Build the static call graph; detect recursion; assign fixed addresses for global‑mode functions.
- [x] **1.7 Code generator** — Translate IR to 8080 assembly. Pattern‑matched instruction selection. Linear‑scan register allocation within basic blocks.
- [x] **1.8 Assembly runtime** — Hand‑written 8080 assembly for `__mul16`, `__div16u`, `__div16s`, `__mod16u`, `__mod16s`, `__shl16`, `__shr16u`, `__shr16s`.
- [x] **1.9 Peephole optimizer** — 10–15 rules covering the most common redundancies.
- [x] **1.10 Assembly output** — Emit a complete `.asm` file for the **v6asm** assembler, targeting Vector 06 (ORG `0x100`).
- [x] **1.11 Test** — Compile and run Sieve of Eratosthenes on an 8080 emulator. Validate correctness. Measure cycle count.

### Phase 2 — Optimizations & Expanded Types

**Goal:** Match or exceed c8080 performance on integer benchmarks.

- [x] **2.1 IR constant folding & propagation** — Evaluate constant expressions at compile time; propagate known values.
- [x] **2.2 Dead‑code elimination** — Remove unreachable code and unused variables/functions.
- [x] **2.3 Strength reduction** — Multiply‑to‑shift, divide‑to‑shift, modulo‑to‑mask for powers of two.
- [x] **2.4 Common sub‑expression elimination** — Within basic blocks initially.
- [x] **2.5 32‑bit integer support** — `long` / `unsigned long` with assembly runtime (`__mul32`, `__div32`, etc.).
- [x] **2.6 Expanded peephole** — 30+ rules; tail‑call optimization; conditional‑branch inversion.
- [x] **2.7 Benchmarking** — Run Dhrystone, Sieve, Fannkuch; compare cycle counts and code size against c8080 and SDCC (Z80‑mode, 8080‑subset only).

### Phase 3 — Structs, Enums, Switch, Arrays

**Goal:** Support idiomatic C programs.

- [ ] **3.1 `struct` and `union`** — Layout, member access, passing by pointer.
- [ ] **3.2 `enum`** — Syntactic sugar over `int`.
- [ ] **3.3 `typedef`** — Type aliases.
- [ ] **3.4 `switch`/`case`** — Jump‑table implementation for dense cases; if‑chain for sparse.
- [ ] **3.5 Multi‑dimensional arrays & initializer lists.**
- [ ] **3.6 Stack mode improvements** — Software frame pointer; efficient SP‑relative access helpers.

### Phase 4 — Standard Library & Vector 06 Target

**Goal:** Usable for real‑world Vector 06 programs.

- [ ] **4.1 Full string.h** — All standard string functions in assembly.
- [ ] **4.2 stdio.h subset** — `printf` (integer formats), `puts`, `getchar`, `putchar`.
- [ ] **4.3 stdlib.h subset** — `malloc`/`free`, `atoi`, `abs`, `rand`.
- [ ] **4.4 Vector 06 target support** — Binary output (ORG `0x100`). Vector 06‑specific I/O stubs (keyboard, display).
- [ ] **4.5 Linker integration** — Support multi‑file compilation; resolve extern symbols across translation units.

### Phase 5 — Advanced Optimizations

**Goal:** Approach the performance of hand‑written assembly.

- [ ] **5.1 Global register allocation** — Whole‑function (or whole‑program) register assignment using linear scan or priority‑based heuristics.
- [ ] **5.2 Loop optimizations** — Loop‑invariant code motion, induction‑variable optimization, loop unrolling for small loops.
- [ ] **5.3 Inline expansion** — Automatic inlining of small functions (configurable threshold).
- [ ] **5.4 Leaf‑function optimization** — Skip any frame setup for functions that make no calls.
- [ ] **5.5 Jump threading & branch optimization** — Eliminate chains of unconditional jumps; invert branch conditions to remove extra jumps.
- [ ] **5.6 Floating‑point support** — Software IEEE‑754 (32‑bit) library in assembly; `float` type in the compiler.
- [ ] **5.7 Variadic functions** — `stdarg.h` support via stack mode.

---

## 4. Project Structure

```
v6_c/
├── plan.md                  # This document
├── Cargo.toml               # Rust project manifest
├── src/
│   ├── main.rs              # Driver: CLI, file I/O, pipeline orchestration
│   ├── lexer.rs             # Tokenizer
│   ├── preproc.rs           # Preprocessor
│   ├── parser.rs            # Recursive-descent parser → AST
│   ├── ast.rs               # AST node definitions and utilities
│   ├── types.rs             # Type system
│   ├── ir.rs                # Three-address IR definitions
│   ├── ir_gen.rs            # AST → IR lowering
│   ├── ir_opt.rs            # IR optimization passes
│   ├── callgraph.rs         # Call-graph analysis, static allocation
│   ├── codegen.rs           # IR → 8080 assembly
│   ├── regalloc.rs          # Register allocator
│   ├── peephole.rs          # Peephole optimizer
│   └── emit.rs              # Assembly text emitter (v6asm format)
├── runtime/
│   ├── mul16.asm             # 16-bit multiply
│   ├── div16.asm             # 16-bit divide/modulo
│   ├── mul32.asm             # 32-bit multiply
│   ├── div32.asm             # 32-bit divide/modulo
│   ├── shift.asm             # Shift routines
│   ├── cmp.asm               # Comparison helpers
│   ├── memcpy.asm            # Memory operations
│   └── crt0.asm              # C runtime startup (Vector 06, ORG 0x100)
├── include/
│   ├── stdint.h
│   ├── stdbool.h
│   ├── stddef.h
│   ├── limits.h
│   ├── string.h
│   ├── stdio.h
│   └── stdlib.h
├── tests/
│   ├── sieve.c               # Sieve of Eratosthenes
│   ├── dhrystone.c            # Dhrystone 2.1
│   ├── fannkuch.c             # Fannkuch benchmark
│   └── unit/                  # Per-feature unit tests
└── README.md
```

---

## 5. Performance Targets

Based on the z88dk benchmark data (scaled to 8080 cycle counts, ~2× Z80 due to missing Z80‑specific optimizations like `IX`/`IY` frame access, `DJNZ`, block instructions):

| Benchmark | Target (cycles) | Est. Time @2 MHz | Rationale |
|---|---|---|---|
| Sieve | ≤ 5,000,000 | ≤ 2.5 s | Beat sccz80; match SDCC/IAR territory via static allocation + peephole. |
| Dhrystone | ≤ 300,000,000 | ≤ 150 s | Competitive with Hitech‑C on pure 8080 (no Z80 extras). |
| Pi (32‑bit) | ≤ 5,000,000,000 | ≤ 42 min | Depends primarily on assembly `__div32`; target z88dk‑small‑int‑math level. |

---

## 6. Design Decisions Summary

| Decision | Rationale |
|---|---|
| Static allocation as default | Biggest single performance win on 8080 (audit §1.1); eliminates ~20 cycles/variable access. |
| Register calling convention | Audit §1.2 shows consistent benefit across all benchmarks (SDCC, z88dk fastcall). |
| Three‑address IR (not stack‑based) | Enables standard optimization passes; avoids sccz80's subroutine‑call overhead for primitives. |
| Assembly runtime library | Audit §1.2: assembly math libs deliver 2–4× speedups — the single most impactful investment after code generation. |
| Peephole optimizer on assembly | Low complexity, high value (audit §1.2 — Hitech‑C's global optimizer is its main advantage). |
| Recursive‑descent parser | Simplicity; no external tools (yacc/bison); easy to extend for new C features. |
| Written in Rust | Memory safety, strong type system, excellent pattern matching for IR/AST transforms, modern tooling (cargo). |
| Separate assembler (v6asm) | Purpose‑built for Vector 06 targets; avoids writing an assembler; focus effort on compilation quality. |
