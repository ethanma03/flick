<#
.SYNOPSIS
  Build Flick and zip a release package into dist\Flick-win-x64.zip.
  Users unzip it and run Install.cmd. The package keeps the same scripts as the repo,
  and install.ps1 notices the binaries next to it instead of in target\release.

.PARAMETER NoBuild  Package the existing target\release binaries.
#>
param([switch]$NoBuild)
$ErrorActionPreference = 'Stop'

$repo = Split-Path $PSScriptRoot -Parent
$stage = Join-Path $repo 'dist\Flick'
$zip = Join-Path $repo 'dist\Flick-win-x64.zip'

if (-not (Test-Path (Join-Path $repo 'vendor\mpv\libmpv-2.dll'))) { & (Join-Path $PSScriptRoot 'fetch-mpv.ps1') }
if (-not $NoBuild) {
    Push-Location $repo
    try { cargo build --release; if ($LASTEXITCODE) { throw 'cargo build failed' } } finally { Pop-Location }
}

if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force (Join-Path $stage 'scripts') | Out-Null
Copy-Item (Join-Path $repo 'target\release\flick.exe'), (Join-Path $repo 'target\release\flickw.exe') $stage
foreach ($s in 'install.ps1', 'set-default.ps1', 'uninstall.ps1') { Copy-Item (Join-Path $PSScriptRoot $s) (Join-Path $stage 'scripts') }
Copy-Item (Join-Path $repo 'packaging\Install.cmd'), (Join-Path $repo 'packaging\Uninstall.cmd'), (Join-Path $repo 'packaging\README.txt') $stage
Copy-Item (Join-Path $repo 'LICENSE') (Join-Path $stage 'LICENSE.txt')
Copy-Item (Join-Path $repo 'vendor\mpv\libmpv-2.dll') $stage
Copy-Item (Join-Path $repo 'vendor\mpv\VERSION') (Join-Path $stage 'VERSION-libmpv.txt')
Copy-Item (Join-Path $repo 'packaging\THIRD-PARTY-NOTICES.txt') $stage

if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $stage -DestinationPath $zip
$size = [math]::Round((Get-Item $zip).Length / 1MB, 1)
Write-Host "Packaged $zip ($size MB)" -ForegroundColor Green
