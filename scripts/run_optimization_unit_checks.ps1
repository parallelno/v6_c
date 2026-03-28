param(
    [string]$Filter,
    [switch]$NoAutoBuildV6asm,
    [switch]$RequireV6asm,
    [switch]$AllowAsmFailure
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$testsDir = Join-Path $repoRoot "tests\unit\optimization"
$outDir = Join-Path $repoRoot "out\tests\unit\optimization"
$toolsDir = Join-Path $repoRoot "dependencies\v6asm"
$v6asmExe = Join-Path $toolsDir "v6asm.exe"
$v6asmSrcDir = Join-Path $toolsDir "v6_assembler"

$strictMode = $RequireV6asm -or (-not $AllowAsmFailure)

function Resolve-V6asm {
    param([switch]$NoAutoBuild)

    if (Test-Path $v6asmExe) {
        return $v6asmExe
    }

    if ($NoAutoBuild) {
        return $null
    }

    New-Item -ItemType Directory -Force -Path $toolsDir | Out-Null

    if (-not (Test-Path $v6asmSrcDir)) {
        git clone https://github.com/parallelno/v6_assembler.git $v6asmSrcDir
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to clone v6_assembler"
        }
    }

    $savedLocation = (Get-Location).Path
    Set-Location $v6asmSrcDir
    try {
        cargo build --release --bin v6asm
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to build v6asm"
        }

        $builtExe = Join-Path $v6asmSrcDir "target\release\v6asm.exe"
        if (-not (Test-Path $builtExe)) {
            throw "Built v6asm executable not found at $builtExe"
        }

        Copy-Item -Path $builtExe -Destination $v6asmExe -Force
    }
    finally {
        if (Test-Path $savedLocation) {
            Set-Location $savedLocation
        }
        else {
            Set-Location $repoRoot
        }
    }

    return $v6asmExe
}

if (-not (Test-Path $testsDir)) {
    throw "Missing tests directory: $testsDir"
}

New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$v6asmCmd = Resolve-V6asm -NoAutoBuild:$NoAutoBuildV6asm
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
        $projectPath = Join-Path $outDir ($base + ".project.json")
        $debugPath = $base + ".debug.json"
        $romPath = $base + ".rom"

        Write-Host "--- $($case.Name)"

        & cargo run --quiet -- $case.FullName -o $asmPath --lst $lstPath
        if ($LASTEXITCODE -ne 0) {
            throw "v6c failed for $($case.Name)"
        }

        if (-not (Test-Path $asmPath)) {
            throw "Missing asm output for $($case.Name): $asmPath"
        }

        $projectObj = @{
            name = $base
            asmPath = ($base + ".asm")
            debugPath = $debugPath
            romPath = $romPath
            cpu = "i8080"
        }
        $projectObj | ConvertTo-Json -Depth 4 | Set-Content -Path $projectPath -Encoding ASCII

        $assembled = $false
        if ($null -ne $v6asmCmd) {
            $innerSavedLocation = (Get-Location).Path
            Set-Location $outDir
            try {
                & $v6asmCmd (Split-Path -Leaf $projectPath)
                if ($LASTEXITCODE -ne 0) {
                    if ($strictMode) {
                        throw "v6asm failed for $($case.Name)"
                    }
                    Write-Warning "v6asm failed for $($case.Name); continuing because -AllowAsmFailure is set"
                }
                else {
                    $assembled = $true
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
