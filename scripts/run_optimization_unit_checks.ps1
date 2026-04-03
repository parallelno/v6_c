param(
    [string]$Filter,
    [switch]$RequireV6asm,
    [switch]$AllowAsmFailure,
    [switch]$UseSmall,
    [switch]$RunExecution,
    [int]$RunCycles = 1000000
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
$v6asmBin = Join-Path $repoRoot "tools\v6asm\v6asm"
$v6emulExe = Join-Path $repoRoot "tools\v6emul\v6emul.exe"
$v6emulBin = Join-Path $repoRoot "tools\v6emul\v6emul"

$strictMode = $RequireV6asm -or (-not $AllowAsmFailure)

function Resolve-V6asm {
    if (Test-Path $v6asmExe) {
        return $v6asmExe
    }
    if (Test-Path $v6asmBin) {
        return $v6asmBin
    }
    return $null
}

function Resolve-V6emul {
    if (Test-Path $v6emulExe) {
        return $v6emulExe
    }
    if (Test-Path $v6emulBin) {
        return $v6emulBin
    }
    return $null
}

function Test-CSourceExecutionReady {
    param([string]$SourcePath)

    $source = Get-Content -Path $SourcePath -Raw
    $mainMatch = [regex]::Match($source, '(?is)\bint\s+main\s*\([^\)]*\)\s*\{(?<body>.*?)\}')
    if (-not $mainMatch.Success) {
        return [PSCustomObject]@{
            IsReady = $false
            Reason = "missing 'int main(...)'"
        }
    }

    if (-not [regex]::IsMatch($mainMatch.Groups['body'].Value, '(?i)\breturn\b')) {
        return [PSCustomObject]@{
            IsReady = $false
            Reason = "main has no return statement"
        }
    }

    return [PSCustomObject]@{
        IsReady = $true
        Reason = ""
    }
}

function Get-OrgAddress {
    param([string]$AsmPath)

    $asm = Get-Content -Path $AsmPath -Raw
    $orgMatch = [regex]::Match($asm, '(?im)^\s*\.ORG\s+(0x[0-9A-Fa-f]+|\d+)')
    if ($orgMatch.Success) {
        return [PSCustomObject]@{
            Found = $true
            Value = $orgMatch.Groups[1].Value
        }
    }

    return [PSCustomObject]@{
        Found = $false
        Value = "0"
    }
}

function Test-AsmExecutionReady {
    param([string]$AsmPath)

    $asm = Get-Content -Path $AsmPath -Raw
    if (-not [regex]::IsMatch($asm, '(?im)^\s*\.ORG\b')) {
        return [PSCustomObject]@{
            IsReady = $false
            Reason = "missing .ORG directive"
        }
    }
    if (-not [regex]::IsMatch($asm, '(?im)^\s*HLT\b')) {
        return [PSCustomObject]@{
            IsReady = $false
            Reason = "missing HLT instruction"
        }
    }

    return [PSCustomObject]@{
        IsReady = $true
        Reason = ""
    }
}

function Parse-HLReturnCode {
    param([string]$Text)

    $cpuLine = [regex]::Match($Text, '(?im)^\s*CPU:.*$')
    if (-not $cpuLine.Success) {
        return $null
    }

    $hlMatch = [regex]::Match($cpuLine.Value, '(?i)\bH=([0-9A-F]{2})\b.*\bL=([0-9A-F]{2})\b')
    if (-not $hlMatch.Success) {
        return $null
    }

    $h = [Convert]::ToInt32($hlMatch.Groups[1].Value, 16)
    $l = [Convert]::ToInt32($hlMatch.Groups[2].Value, 16)
    return ($h * 256 + $l)
}

if (-not (Test-Path $testsDir)) {
    throw "Missing tests directory: $testsDir"
}

New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$v6asmCmd = Resolve-V6asm
$v6emulCmd = Resolve-V6emul
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
Write-Host "Run execution: $RunExecution"
if ($RunExecution) {
    Write-Host "Run cycles: $RunCycles"
}
if ($null -ne $v6asmCmd) {
    Write-Host "Using v6asm: $v6asmCmd"
}
if ($RunExecution -and $null -ne $v6emulCmd) {
    Write-Host "Using v6emul: $v6emulCmd"
}
if ($RunExecution -and $null -eq $v6emulCmd) {
    Write-Warning "Execution requested, but v6emul.exe is missing. Execution step will be skipped."
}

$results = @()

$savedLocation = (Get-Location).Path
Set-Location $repoRoot
try {
    foreach ($case in $cases) {
        $base = [System.IO.Path]::GetFileNameWithoutExtension($case.Name)
        $asmPath = Join-Path $outDir ($base + ".asm")
        $lstPath = Join-Path $outDir ($base + ".v6c.lst")
        $romPath = Join-Path $outDir ($base + ".rom")

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

                    if (-not (Test-Path $romPath)) {
                        throw "v6asm did not emit ROM for $($case.Name): $romPath"
                    }

                    $v6asmLstPath = Join-Path $outDir ($base + ".lst")
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

        $executed = $false
        $resultCode = $null
        $resultText = "SKIP"

        if ($RunExecution) {
            if (-not $assembled) {
                Write-Warning "Skipping execution for $($case.Name): ROM was not assembled"
            }
            elseif ($null -eq $v6emulCmd) {
                Write-Warning "Skipping execution for $($case.Name): missing v6emul.exe"
            }
            else {
                $cValidation = Test-CSourceExecutionReady -SourcePath $case.FullName
                if (-not $cValidation.IsReady) {
                    Write-Warning "Skipping execution for $($case.Name): $($cValidation.Reason)"
                }
                else {
                    $asmValidation = Test-AsmExecutionReady -AsmPath $asmPath
                    if (-not $asmValidation.IsReady) {
                        Write-Warning "Skipping execution for $($case.Name): $($asmValidation.Reason)"
                    }
                    else {
                        $org = Get-OrgAddress -AsmPath $asmPath
                        if (-not $org.Found) {
                            Write-Warning "No .ORG found in $($case.Name); defaulting --load-addr to 0"
                        }

                        $emulOutput = (& $v6emulCmd --rom $romPath --load-addr $org.Value --halt-exit --dump-cpu --run-cycles $RunCycles 2>&1 | Out-String)
                        if ($LASTEXITCODE -ne 0) {
                            throw "v6emul failed for $($case.Name) (exit $LASTEXITCODE):`n$emulOutput"
                        }

                        $parsed = Parse-HLReturnCode -Text $emulOutput
                        if ($null -eq $parsed) {
                            throw "Failed to parse H/L return code for $($case.Name):`n$emulOutput"
                        }

                        $executed = $true
                        $resultCode = $parsed
                        if ($parsed -eq 0) {
                            $resultText = "PASS"
                        }
                        else {
                            $resultText = "FAIL"
                            throw "Execution failed for $($case.Name): HL=$parsed`n$emulOutput"
                        }
                    }
                }
            }
        }

        $results += [PSCustomObject]@{
            Test = $case.Name
            Compiled = $true
            Assembled = $assembled
            Executed = $executed
            Result = $resultText
            ReturnCode = $resultCode
            Asm = $asmPath
            Lst = $lstPath
            Rom = $romPath
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
$results | Format-Table Test, Compiled, Assembled, Executed, Result, ReturnCode, Rom -AutoSize
Write-Host ""
Write-Host "Done. Generated outputs are in: $outDir"
