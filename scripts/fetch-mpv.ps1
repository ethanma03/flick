<#
.SYNOPSIS
  Download libmpv (shinchiro's Windows build) into vendor\mpv. The build copies
  libmpv-2.dll next to the exes, and install.ps1 ships it with Flick.

.PARAMETER Tag     Release tag of shinchiro/mpv-winbuild-cmake (default: the pinned one below).
.PARAMETER Latest  Use the newest release instead of the pinned tag.
#>
param([string]$Tag = '20261021', [switch]$Latest)
$ErrorActionPreference = 'Stop'

$repo = Split-Path $PSScriptRoot -Parent
$dest = Join-Path $repo 'vendor\mpv'
$src = 'shinchiro/mpv-winbuild-cmake'

if ($Latest) { $Tag = (gh release view -R $src --json tagName -q .tagName) }
$asset = gh release view $Tag -R $src --json assets -q '.assets[].name' |
    Where-Object { $_ -like 'mpv-dev-x86_64-2*' } | Select-Object -First 1
if (-not $asset) { throw "No x86_64 mpv-dev archive in release $Tag" }

$tmp = Join-Path ([IO.Path]::GetTempPath()) "flick-mpv-$Tag"
New-Item -ItemType Directory -Force $tmp | Out-Null
Write-Host "Downloading $asset..." -ForegroundColor Cyan
gh release download $Tag -R $src -p $asset -D $tmp --clobber
if ($LASTEXITCODE) { throw 'download failed' }

New-Item -ItemType Directory -Force $dest | Out-Null
# Windows' tar (libarchive) reads 7z.
tar -xf (Join-Path $tmp $asset) -C $dest
if ($LASTEXITCODE) { throw 'extract failed' }
Remove-Item $tmp -Recurse -Force
Set-Content (Join-Path $dest 'VERSION') "$src $Tag $asset"
Write-Host "libmpv ($Tag) is in $dest" -ForegroundColor Green
