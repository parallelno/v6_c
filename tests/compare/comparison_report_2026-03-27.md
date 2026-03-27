# v6c vs c8080 Comparison (dual_compare.c)

Date: 2026-03-27

## Test source

- tests/compare/dual_compare.c

## Commands used

- v6c:
  - cargo run -- tests/compare/dual_compare.c -o tests/compare/dual_compare.v6c.asm
- c8080:
  - references/c8080/c8080.exe -a dual_c8080.asm -o dual_c8080.bin dual_compare.c

## Artifacts

- tests/compare/dual_compare.v6c.asm (4637 bytes)
- tests/compare/dual_c8080.asm (5577 bytes)
- tests/compare/dual_c8080.bin (293 bytes)
- tests/compare/dual_c8080.lst

## High-level metrics (assembly text)

- v6c: 387 total lines, 233 instruction-like lines
- c8080: 304 total lines, 146 instruction-like lines

Function-region instruction counts (approx, label-range based):
- mix:
  - v6c: 132
  - c8080: 48
- main:
  - v6c: 71
  - c8080: 32

## Key observations

1. c8080 emits static stack layout symbols (`__static_stack`, `__s_mix`, `__a_1_mix`, etc.) and heavily reuses them.
2. v6c emits many spill slots (`__spill_0`..`__spill_37`), indicating high spill pressure on this test.
3. c8080 output assembled successfully via sjasmplus and produced a binary.
4. v6c output did not assemble with the same assembler invocation due mnemonic/syntax mismatch (217 errors).

## Calling-convention snippets

c8080 call site and callee setup:
- caller stores first arg and keeps second in HL:
  - `ld (__a_1_mix), hl` then `ld hl, 7` then `call mix`
- callee stores HL as second argument:
  - `ld (__a_2_mix), hl`

v6c call site and callee setup:
- call site initializes two immediates and uses spills before `CALL mix`
- callee starts with:
  - `SHLD _l_mix_x`
  - `SHLD _l_mix_y`

This sequence suggests a potential argument-mapping issue in v6c for two 16-bit params (both stores sourced from HL).

## Practical comparison summary

- For this test case, c8080 produced denser function bodies and directly assemblable output under its bundled toolchain.
- v6c produced larger instruction streams with many spills and currently appears less integrated with the same assembler syntax/target flow.
