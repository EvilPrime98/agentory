<#
.SYNOPSIS
    Build the Agentory Windows installer with Inno Setup.

.DESCRIPTION
    Mirrors packaging/stage.sh + the release workflow: builds the release binary, stages
    dist\Agentory\, then compiles packaging\windows\agentory.iss with ISCC.
    Output: dist\Agentory-Setup-<version>.exe

.PARAMETER Version
    Installer version. Defaults to the version in Cargo.toml.

.PARAMETER SkipBuild
    Skip `cargo build --release` and reuse target\release\agentory.exe.

.PARAMETER IsccPath
    Path to ISCC.exe. Auto-detected (PATH, Program Files, per-user install) when omitted.

.EXAMPLE
    scripts\build-installer.ps1
    scripts\build-installer.ps1 -Version 0.2.0 -SkipBuild
#>
[CmdletBinding()]
param(
    [string]$Version,
    [switch]$SkipBuild,
    [string]$IsccPath
)

$ErrorActionPreference = 'Stop'

$Root  = Split-Path -Parent $PSScriptRoot
$Stage = Join-Path $Root 'dist\Agentory'
$Iss   = Join-Path $Root 'packaging\windows\agentory.iss'

if (-not $Version) {
    $line = Select-String -Path (Join-Path $Root 'Cargo.toml') -Pattern '^version\s*=\s*"(.*)"' | Select-Object -First 1
    if (-not $line) { throw 'Could not read the version from Cargo.toml' }
    $Version = $line.Matches[0].Groups[1].Value
}

function Find-Iscc {
    if ($IsccPath) {
        if (-not (Test-Path $IsccPath)) { throw "ISCC.exe not found at $IsccPath" }
        return $IsccPath
    }
    $cmd = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $candidates = @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    )
    foreach ($c in $candidates) { if (Test-Path $c) { return $c } }
    throw 'ISCC.exe not found. Install Inno Setup 6 (winget install JRSoftware.InnoSetup) or pass -IsccPath.'
}

$iscc = Find-Iscc

if (-not $SkipBuild) {
    Write-Host '==> cargo build --release'
    Push-Location $Root
    try {
        cargo build --release --locked
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    } finally { Pop-Location }
}

$exe = Join-Path $Root 'target\release\agentory.exe'
if (-not (Test-Path $exe)) { throw "$exe not found; run without -SkipBuild" }

Write-Host "==> staging $Stage"
if (Test-Path $Stage) { Remove-Item $Stage -Recurse -Force }
New-Item -ItemType Directory -Path $Stage | Out-Null
Copy-Item $exe $Stage
Copy-Item (Join-Path $Root 'packaging\NOTICE.txt') (Join-Path $Stage 'NOTICE.txt')
$license = Join-Path $Root 'LICENSE'
if (Test-Path $license) { Copy-Item $license $Stage }

Write-Host "==> ISCC $Version"
& $iscc "/DAppVersion=$Version" $Iss
if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }

$out = Join-Path $Root "dist\Agentory-Setup-$Version.exe"
Write-Host "==> installer: $out"
