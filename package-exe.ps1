param(
    [switch]$SkipTests
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $repoRoot

function Run-Step {
    param(
        [string]$Name,
        [scriptblock]$Command
    )

    Write-Host ""
    Write-Host "==> $Name" -ForegroundColor Cyan
    $global:LASTEXITCODE = 0
    & $Command
    $ExitCode = $global:LASTEXITCODE
    if ($ExitCode -ne 0) {
        throw "$Name failed with exit code $ExitCode"
    }
}

Run-Step "Checking pnpm" {
    pnpm --version | Out-Host
}

Run-Step "Checking Node.js" {
    node --version | Out-Host
}

Run-Step "Checking Cargo" {
    cargo --version | Out-Host
}

if (-not $SkipTests) {
    Run-Step "TypeScript typecheck" {
        pnpm typecheck
    }

    Run-Step "Frontend unit tests" {
        pnpm test:unit
    }

    Run-Step "Rust unit tests" {
        cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
    }
}

Run-Step "Building Windows installer" {
    pnpm build
}

$bundleRoot = Join-Path $repoRoot "apps/desktop/src-tauri/target/release/bundle"
$artifacts = @()
if (Test-Path $bundleRoot) {
    $artifacts = Get-ChildItem -Path $bundleRoot -Recurse -File -Include *.exe,*.msi |
        Sort-Object LastWriteTime -Descending
}

Write-Host ""
Write-Host "Build complete." -ForegroundColor Green
if ($artifacts.Count -gt 0) {
    Write-Host "Newest installer artifacts:"
    $artifacts | Select-Object -First 10 | ForEach-Object {
        Write-Host ("  " + $_.FullName)
    }
} else {
    throw "Build reported success but no .exe or .msi artifacts were found under $bundleRoot"
}
