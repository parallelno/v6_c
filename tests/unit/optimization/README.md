# Optimization Unit Tests (C-Only)

This directory contains optimization-focused unit tests as standalone C files.

## Rules

1. Keep tests here, not in Rust source `#[cfg(test)]` blocks.
2. Each file should focus on one optimization family.
3. Each file should include positive, negative, and safety scenarios.
4. Keep tests deterministic and free of runtime I/O dependencies.

## Run All

From repository root:

```powershell
.\scripts\run_optimization_unit_checks.ps1
```

The runner will automatically try to clone and build `v6asm.exe` from:

- `https://github.com/parallelno/v6asm.git`

if `v6asm.exe` is not already available.

## Run One

From repository root:

```powershell
.\scripts\run_optimization_unit_checks.ps1 -Filter opt_ir_strength.c
```

## Optional Flags

1. To allow assembler failures and continue after compile (without strict assembly):

```powershell
.\scripts\run_optimization_unit_checks.ps1 -AllowAsmFailure
```

2. Explicitly force strict mode (same as default):

```powershell
.\scripts\run_optimization_unit_checks.ps1 -RequireV6asm
```

## Output

The runner writes assembly/list/project/debug/rom artifacts to:

- `out/tests/unit/optimization/`

The runner uses pre-built `v6asm.exe` from:

- `tools/v6asm/`

## Assembly Validation

The runner enforces successful assembly in strict mode (default).
