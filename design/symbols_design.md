# v6c Debug Symbols Design

## Summary

v6c emits a `.symbols.json` file alongside the `.asm` output.
The format is a strict superset of the [v6asm symbols format](v6asm_symbols_design.md):
every field defined there keeps the same name, type, and semantics.
v6c adds new sections and symbol types to cover C-level concepts—variables,
types, inlined code, and optimized-out definitions—while remaining
readable by existing v6asm-aware tooling that simply ignores unknown keys.

---

## Design Goals

| # | Goal |
|---|------|
| G1 | **Full compatibility** with v6asm `.symbols.json`; a consumer that only knows v6asm fields must still work. |
| G2 | **Map every emitted instruction back to a C source line** (file + line). |
| G3 | **Preserve function identity** even after inlining and specialization. |
| G4 | **Describe optimized-out variables** so a debugger can explain "this value was constant-folded to X" or "this variable was removed by dead-code elimination". |
| G5 | **Describe inlined call sites** so a debugger can show an inlined call stack. |
| G6 | **Keep the file small**; only emit sections that carry data. |

---

## Compatibility Rules

1. The top-level keys `symbols`, `lineAddresses`, and `dataLines` retain their v6asm schema exactly.
2. New top-level keys (`functions`, `inlineFrames`, `variables`, `sourceFiles`) are additive. Old consumers ignore them.
3. The `type` field in a symbol entry may carry new values (`"var"`, `"param"`, `"typedef"`). Old consumers treat unknown types as opaque strings.
4. Numeric values remain plain JSON numbers (0–65535 for addresses, –1 for "not applicable").

---

## New Symbol Types

| Type | Meaning |
|------|---------|
| `"func"` | C function (compatible with v6asm) |
| `"label"` | Assembly-level label (compatible with v6asm) |
| `"const"` | `#define` or `enum` constant (compatible with v6asm) |
| `"var"` | **New.** C global or static variable. |
| `"param"` | **New.** C function parameter. |

---

## Extended Top-Level Structure

```jsonc
{
  // ── v6asm-compatible sections ──────────────────────────────

  "symbols":        { /* unchanged schema */ },
  "lineAddresses":  { /* unchanged schema */ },
  "dataLines":      { /* unchanged schema */ },

  // ── v6c extensions ─────────────────────────────────────────

  "sourceFiles":    { /* §1 */ },
  "functions":      { /* §2 */ },
  "inlineFrames":   { /* §3 */ },
  "variables":      { /* §4 */ }
}
```

Sections are omitted when empty (Goal G6).

---

## §1 `sourceFiles` — Source File Table

Maps a short numeric ID to each compilation-unit-relative path.
All other sections reference files by ID to reduce JSON size.

```jsonc
{
  "sourceFiles": {
    "0": "main.c",
    "1": "util.c",
    "2": "include/stddef.h"
  }
}
```

Rule: paths are project-relative, forward-slash only, no drive letters.

---

## §2 `functions` — Function Metadata

One entry per original C function (including functions that were fully
inlined away and no longer exist in the output).

```jsonc
{
  "functions": {
    "main": {
      "addr":       256,          // entry address (null if fully inlined away)
      "endAddr":    310,          // first address past function body (null if inlined away)
      "file":       0,            // sourceFiles ID
      "line":       5,            // definition line (1-based)
      "params":     ["argc", "argv"],
      "inlined":    false,        // true ⇒ no standalone body exists
      "specialized": false        // true ⇒ this is a __spec_ variant
    },
    "add": {
      "addr":       null,
      "endAddr":    null,
      "file":       1,
      "line":       12,
      "params":     ["a", "b"],
      "inlined":    true,
      "specialized": false
    },
    "__spec_draw_1": {
      "addr":       400,
      "endAddr":    430,
      "file":       1,
      "line":       30,
      "params":     ["mode"],
      "inlined":    false,
      "specialized": true,
      "origin":     "draw"        // original function name before specialization
    }
  }
}
```

### Inlined Functions

When the optimizer inlines a function, the function entry gets
`"inlined": true` and `addr`/`endAddr` become `null`.
The body's instructions still carry their original source line numbers
(preserved by `inline_expand()` in `ir_opt.rs`), so `lineAddresses`
continues to reference the callee's source file and line.

### Specialized Functions

`function_specialization()` creates copies named `__spec_<name>_<N>`.
These get their own `functions` entry with `"specialized": true` and
an `"origin"` field pointing to the original function name.

---

## §3 `inlineFrames` — Inline Call-Site Map

Describes where inlined code lives inside its caller.
One entry per inline expansion.

```jsonc
{
  "inlineFrames": [
    {
      "callee":     "add",          // inlined function name
      "caller":     "main",         // function that contains the inlined body
      "callFile":   0,              // sourceFiles ID of the call site
      "callLine":   18,             // line of the call expression
      "addrStart":  270,            // first address of inlined body in caller
      "addrEnd":    280             // first address past inlined body
    }
  ]
}
```

### How the Debugger Uses This

When the program counter is inside `[addrStart, addrEnd)`:

1. Look up `lineAddresses` → get the callee's source file + line.
2. Look up `inlineFrames` → find the call site in the caller.
3. Present a virtual call stack:
   ```
   add()       at util.c:14      ← callee source line
   main()      at main.c:18      ← call site
   ```

For nested inlining (A inlines B which inlines C), multiple
`inlineFrames` entries overlap; the debugger walks the chain
`callee → caller` until a non-inlined function is found.

### Implementation Note

During `inline_expand()`, the compiler already knows:
- the call site IR instruction (carries `line` for `callLine`),
- the callee name,
- the caller name,
- the first and last remapped label (convertible to address range during emit).

A new `InlineRecord` struct will collect this data at inline-expansion
time and survive through codegen into the emitter.

---

## §4 `variables` — Variable Descriptions

Describes C-level variables (globals, statics, and function parameters)
with their storage location or, if optimized away, the reason and
residual value.

```jsonc
{
  "variables": {
    "_g_counter": {
      "file":     0,
      "line":     3,
      "ctype":    "int",
      "storage":  "static",       // "static" | "stack" | "register" | "optimized_out"
      "addr":     32768,          // memory address (when storage is "static")
      "scope":    "global"        // "global" | "<functionName>"
    },
    "_l_main_i": {
      "file":     0,
      "line":     7,
      "ctype":    "int",
      "storage":  "static",
      "addr":     32770,
      "scope":    "main"
    },
    "_l_main_limit": {
      "file":     0,
      "line":     8,
      "ctype":    "int",
      "storage":  "optimized_out",
      "addr":     null,
      "scope":    "main",
      "constValue": 100,
      "reason":   "constant_folded"
    }
  }
}
```

### Storage Classes

| `storage` | Meaning | `addr` |
|-----------|---------|--------|
| `"static"` | Statically allocated in memory (global mode). | Memory address. |
| `"stack"` | Allocated on the 8080 stack at runtime (stack mode / recursive). | SP offset (signed). |
| `"register"` | Lives only in a physical register (rare, short-lived). | `null` |
| `"optimized_out"` | Removed by the optimizer. | `null` |

### Optimized-Out Variables

When a variable is removed, the entry explains what happened:

| `reason` | Meaning | Extra field |
|----------|---------|-------------|
| `"constant_folded"` | Value was a compile-time constant; every use was replaced by the literal. | `"constValue": <number>` |
| `"dead_code"` | Variable was written but never read. | — |
| `"inlined_away"` | Variable belonged to a function that was fully inlined; its storage merged into callers. | `"inlinedInto": "<caller>"` |
| `"strength_reduced"` | Original variable replaced by a derived induction variable. | `"replacedBy": "<new_label>"` |

This lets the debugger show meaningful hover text like:
> `limit` — optimized out (constant folded to 100)

---

## Interaction with the Existing Pipeline

### Data Flow

```
source.c
  │
  ▼
parser/AST ──► ir_gen ──────► IR (each IrInstr carries .line)
                                │
                                ▼
                            ir_opt
                   ┌────────────┤
                   │  inline_expand() ──► InlineRecord {callee, caller, callLine, labels}
                   │  constant_fold()  ──► mark variables as optimized_out
                   │  dce()            ──► mark variables as dead_code
                   │  function_specialization() ──► __spec_ entries
                   │
                   ▼
               codegen ──► assembly lines with ; C_LINE markers
                   │
                   ▼
               emit
                   ├──► .asm file  (unchanged)
                   ├──► .lst file  (unchanged)
                   └──► .symbols.json  (NEW)
```

### New Internal Structures

```
InlineRecord {
    callee:     String,
    caller:     String,
    call_line:  u32,
    call_file:  String,       // relative path
    start_label: LabelId,     // first label of inlined body
    end_label:   LabelId,     // merge label after inlined body
}

VariableInfo {
    name:        String,
    file:        String,
    line:        u32,
    ctype:       String,      // display string, e.g. "int", "char *"
    scope:       String,      // "global" or function name
    storage:     Storage,     // enum { Static(u16), Stack(i16), Register, OptimizedOut }
    opt_reason:  Option<OptReason>,
    const_value: Option<i64>,
}
```

These are collected during IR optimization and codegen, then serialized
in `emit.rs` after address resolution (so label IDs become concrete
addresses).

---

## CLI Integration

New output path derived automatically:

| Input | Output |
|-------|--------|
| `v6c main.c -o main.asm` | `main.symbols.json` (beside `main.asm`) |
| `v6c main.c -o out/prog.asm` | `out/prog.symbols.json` |

A future `--no-symbols` flag may suppress generation; by default the
file is always emitted.

---

## Example Artifact

Compiling:

```c
// main.c
int counter;
int add(int a, int b) { return a + b; }
int main() {
    int limit = 100;
    counter = add(2, 3);
    return counter - limit;
}
```

With `add` inlined and `limit` constant-folded:

```json
{
  "sourceFiles": {
    "0": "main.c"
  },
  "symbols": {
    "main":       { "value": 256,   "path": "main.c", "line": 4, "type": "func"  },
    "_g_counter": { "value": 32768, "path": "main.c", "line": 2, "type": "var"   },
    "add":        { "value": -1,    "path": "main.c", "line": 3, "type": "func"  }
  },
  "lineAddresses": {
    "main.c": {
      "3": [270, 274],
      "4": [256],
      "6": [270],
      "7": [280]
    }
  },
  "dataLines": {
    "main.c": {
      "2": { "addr": 32768, "byteLength": 2, "unitBytes": 2 }
    }
  },
  "functions": {
    "main": {
      "addr": 256,
      "endAddr": 290,
      "file": 0,
      "line": 4,
      "params": [],
      "inlined": false,
      "specialized": false
    },
    "add": {
      "addr": null,
      "endAddr": null,
      "file": 0,
      "line": 3,
      "params": ["a", "b"],
      "inlined": true,
      "specialized": false
    }
  },
  "inlineFrames": [
    {
      "callee": "add",
      "caller": "main",
      "callFile": 0,
      "callLine": 6,
      "addrStart": 270,
      "addrEnd": 280
    }
  ],
  "variables": {
    "_g_counter": {
      "file": 0,
      "line": 2,
      "ctype": "int",
      "storage": "static",
      "addr": 32768,
      "scope": "global"
    },
    "_l_main_limit": {
      "file": 0,
      "line": 5,
      "ctype": "int",
      "storage": "optimized_out",
      "addr": null,
      "scope": "main",
      "constValue": 100,
      "reason": "constant_folded"
    },
    "_p_add_a": {
      "file": 0,
      "line": 3,
      "ctype": "int",
      "storage": "optimized_out",
      "addr": null,
      "scope": "add",
      "constValue": 2,
      "reason": "constant_folded"
    },
    "_p_add_b": {
      "file": 0,
      "line": 3,
      "ctype": "int",
      "storage": "optimized_out",
      "addr": null,
      "scope": "add",
      "constValue": 3,
      "reason": "constant_folded"
    }
  }
}
```

---

## Implementation Plan

### Phase 1 — Skeleton + `symbols` + `lineAddresses` (minimal viable)

1. Add `SymbolsOutput` struct in `emit.rs` with serialization to JSON.
2. Populate `symbols` from:
   - Function labels → `"func"`.
   - Global/static variable labels → `"var"`.
   - `#define` numeric constants → `"const"`.
3. Populate `lineAddresses` from existing `; C_LINE` markers and
   resolved addresses already computed for the listing pass.
4. Write `.symbols.json` in the emit phase after address resolution.
5. Derive output path from `CompilerOpts.output` (replace `.asm` → `.symbols.json`).

### Phase 2 — `functions` + `dataLines`

1. Collect function entry/end addresses during codegen (first/last
   label address per function).
2. Track parameter names from AST function definitions.
3. Emit `dataLines` for `.DB`/`.DW` directives generated for globals
   and string literals.
4. Mark inlined functions (`addr: null`, `inlined: true`).

### Phase 3 — `inlineFrames`

1. Add `InlineRecord` struct.
2. Collect records in `inline_expand()` — callee name, caller name,
   call-site line, start/end labels.
3. Resolve labels to addresses in the emit phase.
4. Serialize into `inlineFrames` array.

### Phase 4 — `variables` + optimized-out tracking

1. Add `VariableInfo` struct.
2. During `ir_opt` passes, tag variables when:
   - `constant_fold()` replaces all uses with a literal.
   - `dce()` removes a write with no readers.
   - `inline_expand()` merges a function's locals into the caller.
   - `strength_reduce()` replaces a multiplication IV.
3. After callgraph allocation, fill in `addr` for surviving static variables.
4. Serialize into `variables` section.

### Phase 5 — `sourceFiles` + multi-file support

1. Assign numeric IDs to each source file encountered during preprocessing.
2. Carry file ID alongside line numbers through IR → codegen → emit.
3. Convert `functions`, `inlineFrames`, `variables` from path strings to file IDs.

---

## Testing Strategy

### Unit Tests

| Test | Assertion |
|------|-----------|
| Serialize empty program | JSON has `symbols: {}`, `lineAddresses: {}`, no other sections. |
| Single function, no optimization | `functions` entry with correct `addr`, `endAddr`, `params`. |
| `#define` constant | Appears in `symbols` as `"const"`. |
| Global variable | Appears in both `symbols` (type `"var"`) and `variables` (storage `"static"`). |
| Inlined function | `functions` entry has `inlined: true`, `addr: null`. |
| Inline frame | `inlineFrames` entry has correct caller/callee/line/addr range. |
| Constant-folded variable | `variables` entry: `storage: "optimized_out"`, `reason: "constant_folded"`, `constValue` matches. |
| DCE-removed variable | `storage: "optimized_out"`, `reason: "dead_code"`. |
| Specialized function | `functions` entry: `specialized: true`, `origin` matches original name. |
| Multi-file | `sourceFiles` has entries for each included file; symbols reference correct IDs. |
| Backward-compat | JSON parsed by v6asm-only consumer succeeds; unknown keys are ignored. |

### Integration Tests

| Test | Assertion |
|------|-----------|
| Round-trip: compile → assemble → load symbols | All addresses in `lineAddresses` fall within known assembled ROM range. |
| Debugger stack trace for inlined code | `inlineFrames` chain produces correct virtual call stack. |
| Hover on optimized-out variable | `variables` entry provides `constValue` or `reason`. |

### Regression Tests

| Test | Assertion |
|------|-----------|
| Missing `dataLines` section | Consumer handles absent section gracefully. |
| Missing `inlineFrames` section | Consumer falls back to flat source mapping. |
| Empty `variables` | Section omitted from JSON entirely (Goal G6). |
