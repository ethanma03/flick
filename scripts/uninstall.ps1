<#
.SYNOPSIS  Remove Flick's install folder, PATH entry and per-user registrations.
#>
$ErrorActionPreference = 'Continue'
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Flick'

if (Test-Path "$dest\flick.exe") { & "$dest\flick.exe" quit 2>$null | Out-Null }
Start-Sleep -Milliseconds 500
Get-Process flickw -ErrorAction SilentlyContinue | Stop-Process -Force

foreach ($k in 'Software\Classes\Flick.Video', 'Software\Classes\Flick.Audio', 'Software\Classes\Applications\flickw.exe',
               'Software\Classes\Directory\shell\Flick.Play', 'Software\Flick') {
    Remove-Item "HKCU:\$k" -Recurse -Force -ErrorAction SilentlyContinue
}
Remove-ItemProperty 'HKCU:\Software\RegisteredApplications' -Name 'Flick' -ErrorAction SilentlyContinue
Get-ChildItem 'HKCU:\Software\Classes' -ErrorAction SilentlyContinue | Where-Object { $_.PSChildName -like '.*' } | ForEach-Object {
    Remove-ItemProperty "$($_.PSPath)\OpenWithProgids" -Name 'Flick.Video', 'Flick.Audio' -ErrorAction SilentlyContinue
}

Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'Flick' -ErrorAction SilentlyContinue
$policy = (Get-ItemProperty 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\System' -ErrorAction SilentlyContinue).DefaultAssociationsConfiguration
if ($policy -and (Test-Path $policy) -and ((Get-Content -Raw $policy) -match 'Flick\.')) {
    & (Join-Path $PSScriptRoot 'set-default.ps1') -Remove
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
[Environment]::SetEnvironmentVariable('Path', (($userPath -split ';') | Where-Object { $_ -and $_ -ne $dest }) -join ';', 'User')
Remove-Item $dest -Recurse -Force -ErrorAction SilentlyContinue
Write-Host 'Flick uninstalled. (Settings and resume positions remain in %LOCALAPPDATA%\Flick.)'
