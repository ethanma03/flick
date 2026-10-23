<#
.SYNOPSIS
  Pull the latest source, rebuild, reinstall, and reopen whatever was playing at the same spot.
  File associations survive because the install path never changes.
#>
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$cli = Join-Path $env:LOCALAPPDATA 'Programs\Flick\flick.exe'

Push-Location $repo
try {
    if (git remote) {
        Write-Host 'Pulling...' -ForegroundColor Cyan
        git pull --ff-only
        if ($LASTEXITCODE) { throw 'git pull failed' }
    }
    $last = $null
    if (Test-Path $cli) {
        try { $last = & $cli state 2>$null | ConvertFrom-Json } catch { }
    }
    & (Join-Path $PSScriptRoot 'install.ps1') -NoSettings
    if ($last -and $last.file) {
        $openArgs = @('open', $last.file)
        if ($last.position) { $openArgs += @('--at', [string]$last.position) }
        if ($last.paused) { $openArgs += '--paused' }
        & $cli @openArgs | Out-Null
    }
    Write-Host "Flick updated: $(& $cli --version)" -ForegroundColor Green
} finally {
    Pop-Location
}

