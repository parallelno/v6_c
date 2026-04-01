param(
    [string]$Filter,
    [switch]$RequireV6asm,
    [switch]$AllowAsmFailure,
    [switch]$UseSmall
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if ($UseSmall) {
    $testsDir = Join-Path $repoRoot "tests\unit\optimization_small"
    $outDir = Join-Path $repoRoot "out\tests\unit\optimization_small"
} else {
    $testsDir = Join-Path $repoRoot "tests\unit\optimization"
    $outDir = Join-Path $repoRoot "out\tests\unit\optimization"
}
$v6asmExe = Join-Path $repoRoot "tools\v6asm\v6asm.exe"

$strictMode = $RequireV6asm -or (-not $AllowAsmFailure)

function Resolve-V6asm {
    if (Test-Path $v6asmExe) {
        return $v6asmExe
    }
    return $null
}

if (-not (Test-Path $testsDir)) {
    throw "Missing tests directory: $testsDir"
}

New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$v6asmCmd = Resolve-V6asm
if ($null -eq $v6asmCmd -and $strictMode) {
    throw "Strict mode requires v6asm, but it was not found and auto-build is disabled"
}

$cases = @(Get-ChildItem -Path $testsDir -Filter "*.c" | Sort-Object Name)
if ($Filter) {
    $cases = @($cases | Where-Object { $_.Name -like $Filter })
}

if ($null -eq $cases -or $cases.Count -eq 0) {
    throw "No test cases matched filter '$Filter' in $testsDir"
}

Write-Host "Building optimization test set from $testsDir"
Write-Host "Output directory: $outDir"
Write-Host "Strict assembler mode: $strictMode"
if ($null -ne $v6asmCmd) {
    Write-Host "Using v6asm: $v6asmCmd"
}

$results = @()

$savedLocation = (Get-Location).Path
Set-Location $repoRoot
try {
    foreach ($case in $cases) {
        $base = [System.IO.Path]::GetFileNameWithoutExtension($case.Name)
        $asmPath = Join-Path $outDir ($base + ".asm")
        $lstPath = Join-Path $outDir ($base + ".v6c.lst")
        $romPath = $base + ".rom"

        Write-Host "--- $($case.Name)"

        & cargo run --quiet -- $case.FullName -o $asmPath --lst $lstPath
        if ($LASTEXITCODE -ne 0) {
            throw "v6c failed for $($case.Name)"
        }

        if (-not (Test-Path $asmPath)) {
            throw "Missing asm output for $($case.Name): $asmPath"
        }

        if (-not (Test-Path $lstPath)) {
            throw "Missing v6c lst output for $($case.Name): $lstPath"
        }

        $assembled = $false
        if ($null -ne $v6asmCmd) {
            $innerSavedLocation = (Get-Location).Path
            Set-Location $outDir
            try {
                & $v6asmCmd (Split-Path -Leaf $asmPath) --lst
                if ($LASTEXITCODE -ne 0) {
                    if ($strictMode) {
                        throw "v6asm failed for $($case.Name)"
                    }
                    Write-Warning "v6asm failed for $($case.Name); continuing because -AllowAsmFailure is set"
                }
                else {
                    $assembled = $true

                    $romOutput = Join-Path $outDir ($base + ".rom")
                    $v6asmLstPath = Join-Path $outDir ($base + ".lst")

                    if (-not (Test-Path $romOutput)) {
                        throw "v6asm did not emit ROM for $($case.Name): $romOutput"
                    }
                    if (-not (Test-Path $v6asmLstPath)) {
                        throw "v6asm did not emit list file for $($case.Name): $v6asmLstPath"
                    }
                }
            }
            finally {
                if (Test-Path $innerSavedLocation) {
                    Set-Location $innerSavedLocation
                }
                else {
                    Set-Location $repoRoot
                }
            }
        }

        $results += [PSCustomObject]@{
            Test = $case.Name
            Asm = $asmPath
            Lst = $lstPath
            Rom = (Join-Path $outDir $romPath)
            Assembled = $assembled
        }
    }
}
finally {
    if (Test-Path $savedLocation) {
        Set-Location $savedLocation
    }
    else {
        Set-Location $repoRoot
    }
}

Write-Host ""
Write-Host "Optimization unit compile/check summary:"
$results | Format-Table Test, Assembled, Rom -AutoSize
Write-Host ""
Write-Host "Done. Generated outputs are in: $outDir"
