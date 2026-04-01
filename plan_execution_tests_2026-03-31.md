# Plan: Integrating v6emul for ROM Execution Verification

**Date:** 2026-03-31

## Problem

The current test suite only checks compilation success and assembly patterns — it never executes the generated ROM to verify correctness of the compiled code.

## Solution

Use the v6emul CLI emulator's headless test mode (`--halt-exit --dump-cpu`) to execute compiled ROMs and verify results.

## 1. Test Convention: How Test Programs Communicate Results

**Primary mechanism — main() return value:**
- `crt0.asm` already calls `main` then `HLT`. The return value lives in **HL**.
- v6emul's `--halt-exit --dump-cpu` prints HL on exit.
- **Convention**: `return 0` = all checks passed, `return N` = check N failed.

**Secondary mechanism — TEST_OUT port (0xED):**
- v6emul captures `OUT 0xED` in test mode as `TEST_OUT port=0xED value=XX`.
- Test programs can emit per-check results via inline asm `OUT 0xED`.
- This gives finer-grained diagnostics without needing to rely solely on the final return code.

**Example test file pattern:**
```c
// tests/unit/arith.c
int add(int a, int b) { return a + b; }
int mul(int a, int b) { return a * b; }

int main() {
    if (add(2, 3) != 5) return 1;
    if (mul(6, 7) != 42) return 2;
    if (add(-1, 1) != 0) return 3;
    return 0;
}
```

## 2. Tool Dependencies

All external tools are pre-built executables stored in the `tools/` directory:

```
tools/
    v6asm/v6asm.exe      # assembler (Intel 8080)
    v6emul/v6emul.exe    # emulator (Vector-06C, headless test mode)
    c8080/c8080.exe      # reference C compiler (for comparison)
```

No auto-build or clone step needed. The test runner checks for the executable and skips the corresponding step with a warning if it's missing. When a tool is updated, simply replace the `.exe` in the `tools/` directory.

## 3. Full Test Pipeline (Per Test File)

```
  source.c ──v6c──► source.asm ──v6asm──► source.rom ──v6emul──► result
     │                  │                     │                    │
  compile           assemble             execute              verify
  (exists)          (exists)             (NEW)                (NEW)
```

Each step:
1. **Compile**: `v6c test.c -o test.asm`
2. **Assemble**: `v6asm test.asm` → `test.rom`
3. **Extract load address**: Parse `.ORG` directive from `test.asm` (see section 7)
4. **Execute**: `v6emul --rom test.rom --load-addr <org_addr> --halt-exit --dump-cpu --run-cycles 1000000`
5. **Verify**: Parse stdout — check `H=00 L=00` (return 0 = pass). The `--run-cycles` acts as a safety timeout to catch infinite loops.

## 4. Where to Integrate

**Extend the existing PowerShell runner**
Add an execution phase to `scripts/run_optimization_unit_checks.ps1`:
- After successful assembly, if `v6emul.exe` exists, run the ROM
- Parse `--dump-cpu` output for HL value
- Report per-test: `Compiled | Assembled | Executed | Result`
- New flag: `-RunExecution` (opt-in initially, so existing tests aren't broken)

## 5. Rust Integration Test Structure

```rust
// For each .c file in tests/unit/ (recursively):
//   1. Compile with v6c → .asm
//   2. Validate .asm (see section 8)
//   3. Assemble with v6asm → .rom
//   4. Execute with v6emul --halt-exit --dump-cpu --run-cycles <limit>
//   5. Parse HL register from output
//   6. Assert HL == 0
```

Auto-discover all `.c` files under `tests/unit/` (like the existing PowerShell runner does) rather than hard-coding each test.

## 6. Output Parsing

v6emul `--dump-cpu` output format:
```
HALT at PC=0x0105 after 847231 cpu_cycles 1200 frames
CPU: A=42 F=00 B=01 C=02 D=03 E=04 H=00 L=00
     PC=0105 SP=C000 CC=847231
```

Parse: extract `H` and `L` values → combine to 16-bit return code. `0x0000` = pass.

## 7. Handling Load Address

Currently the emitter writes `.ORG 0x100` (the standard Vector-06C load address), confirmed in `src/emit.rs` line 557 and `runtime/crt0.asm` comments. In the future the ORG address may become a compiler flag.

**Approach:** The test runner must parse the generated `.asm` file for the first `.ORG` directive and extract the address dynamically, then pass it as `--load-addr <addr>` to v6emul.
If `.ORG` directive is not found, the start address is 0. This avoids hard-coding an address that may change.

Example (pseudo-code):
```
org_addr = regex_match(asm_content, /\.ORG\s+(0x[0-9A-Fa-f]+|\d+)/)
v6emul --rom test.rom --load-addr {org_addr} --halt-exit --dump-cpu --run-cycles 1000000
```

## 8. Test Validation and Migration

All `.c` files under `tests/unit/` are candidates for ROM execution testing. Before executing a ROM, the test runner must validate that the test is well-formed.

### Pre-execution validation (on the C source)

The runner checks each `.c` file for:
- `int main(` — must have a main function that returns int (not `void main`)
- `return` — must have at least one return statement in main

If validation fails, the test is **skipped with a warning**, not failed. This allows gradual migration.

### Post-compilation validation (on the generated .asm)

After compilation, the runner checks the generated `.asm` for:
- `.ORG` — must have a load address directive (used to determine `--load-addr`)
- `HLT` — must have a halt instruction (so the emulator stops via `--halt-exit`)

If validation fails, the ROM execution step is **skipped with a warning**.

### Migration of existing tests

All existing C unit tests currently use `void main(void)` with no return values. They compute results into global variables but don't signal pass/fail. Each test must be migrated to:
1. Change `void main(void)` → `int main()`
2. Add assertions as `if (expected != actual) return N;` where N identifies the failing check
3. Add `return 0;` at the end (all checks passed)

Migration can be done incrementally — the validation step ensures only migrated tests run through the emulator.

## 9. Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| Infinite loop in test ROM | `--run-cycles 1000000` as timeout; treat timeout as failure |
| v6emul.exe missing from tools/ | Test fail if v6emul not found. Output the issue. |
| Load address mismatch | Parse `.ORG` from .asm dynamically (section 7) |

## 10. Implementation Order

1. Verify v6emul works: manually run `tools/v6emul/v6emul.exe` with a hand-crafted ROM
2. Migrate `tests/unit/arith.c` to `int main()` with return-value checks, manually run the full chain
3. Verify the load address and `--dump-cpu` output parsing
4. Create the Rust integration test (`tests/execution.rs`) with validation logic (section 8)
5. Migrate remaining C unit tests incrementally (add `int main()` + return checks)
6. Run `cargo test` to validate everything end-to-end
